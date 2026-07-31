import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

// MARK: - Rust-owned session capacity

final class RuntimeSessionCapacityRetirementTests: RuntimeNappletSessionTestCase {
    func testSixteenActiveWrappersOccupyEveryLaunchSlot() throws {
        let fixture = try makeCapacityFixture("active-capacity")
        defer { fixture.profile.close() }

        let runtimes = try launchMaximumSessions(fixture)

        XCTAssertEqual(runtimes.count, NativeRuntimeLibraryLimits.maximumSessions)
        XCTAssertEqual(
            fixture.profile.reserveSessionLaunch(),
            .capacity(maximum: NativeRuntimeLibraryLimits.maximumSessions)
        )
        XCTAssertEqual(Set(runtimes.map(\.sessionID)).count, runtimes.count)
    }

    func testSixteenRustTerminatedWrappersReleaseEveryLaunchSlot() throws {
        let fixture = try makeCapacityFixture("rust-terminal-capacity")
        defer { fixture.profile.close() }
        let runtimes = try launchMaximumSessions(fixture)
        let responses = CapacityRetirementResponses()
        runtimes[0].setResponseSink(responses.append)

        for runtime in runtimes {
            fixture.profile.controller.stop(sessionId: runtime.sessionID)
        }
        let ended = try fixture.profile.snapshotForTesting
        XCTAssertTrue(ended.sessions.isEmpty)
        XCTAssertEqual(
            fixture.profile.reserveSessionLaunch(),
            .capacity(maximum: NativeRuntimeLibraryLimits.maximumSessions),
            "absent Rust rows stay bounded until their terminal frame is delivered"
        )

        let responseJSON =
            #"{"type":"outbox.publish.result","id":"rust-terminal","ok":false,"error":"session stopped"}"#
        fixture.profile.update(
            frame: terminalFrame(
                profile: fixture.profile,
                snapshot: ended,
                runtimes: runtimes,
                responseJSON: responseJSON
            )
        )

        XCTAssertEqual(responses.values, [Data(responseJSON.utf8)])
        XCTAssertEqual(runtimes.count, NativeRuntimeLibraryLimits.maximumSessions)
        XCTAssertTrue(
            runtimes.allSatisfy { $0.stopTerminalEvidence() == .delivered }
        )
        fixture.profile.lock.lock()
        let registeredCount = fixture.profile.sessions.count
        let stoppingCount = fixture.profile.stoppingSessions.count
        fixture.profile.lock.unlock()
        XCTAssertEqual(registeredCount, 0)
        XCTAssertEqual(stoppingCount, 0)
        assertEveryLaunchSlotAvailable(fixture.profile)
    }

    private typealias CapacityFixture = (
        profile: NativeRuntimeProfile,
        installed: NativeRuntimeInstalledArtifact
    )

    private func makeCapacityFixture(_ name: String) throws -> CapacityFixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-\(name)-\(UUID().uuidString)",
                isDirectory: true
            )
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        try quiesceAutomaticObservation(profile)
        let published = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let installed = try profile.installSignedNamed(
            title: "Capacity Retirement",
            eventJSON: Data(
                contentsOf: published.appendingPathComponent("event.json")
            ),
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [
                indexDigest: Data(
                    contentsOf: published.appendingPathComponent("index.html")
                ),
            ]
        )
        let review = try XCTUnwrap(
            profile.permissionReview(for: installed.permissionCoordinate).review
        )
        let grant = profile.applyPermissionDecisions(
            NativeRuntimePermissionDecisionBatch(
                coordinate: installed.permissionCoordinate,
                reviewRevision: review.revision,
                decisions: review.capabilities.map {
                    NativeRuntimePermissionDecisionSelection(
                        domain: $0.domain,
                        decision: .allowExactBuild
                    )
                }
            )
        )
        XCTAssertTrue(grant.applied)
        return (profile, installed)
    }

    private func launchMaximumSessions(
        _ fixture: CapacityFixture
    ) throws -> [RustRuntimeNappletSession] {
        try (0 ..< NativeRuntimeLibraryLimits.maximumSessions).map { _ in
            let artifact = try fixture.profile.launchInstalled(fixture.installed)
            return try XCTUnwrap(
                artifact.runtimeSession as? RustRuntimeNappletSession
            )
        }
    }

    private func quiesceAutomaticObservation(
        _ profile: NativeRuntimeProfile
    ) throws {
        let entered = expectation(description: "observer callback entered")
        let returned = expectation(description: "observer callback returned")
        let allowReturn = DispatchSemaphore(value: 0)
        let library = try profile.observeInstalledLibrary { update in
            guard case .next = update else { return }
            entered.fulfill()
            _ = allowReturn.wait(timeout: .now() + 5)
            returned.fulfill()
        }
        profile.setInstalledLibraryFilter("capacity-observer-barrier")
        wait(for: [entered], timeout: 2)
        profile.lock.lock()
        let observation = profile.observation
        profile.observation = nil
        profile.lock.unlock()
        observation?.stop()
        allowReturn.signal()
        wait(for: [returned], timeout: 2)
        library.cancel()
    }

    private func terminalFrame(
        profile: NativeRuntimeProfile,
        snapshot: RuntimeSnapshot,
        runtimes: [RustRuntimeNappletSession],
        responseJSON: String
    ) -> RuntimeObservationFrame {
        var events = [
            RuntimeEvent(
                sequence: 1,
                kind: "envelope-handled",
                detail: "queued terminal response",
                sessionId: runtimes[0].sessionID,
                responseJson: responseJSON
            ),
        ]
        events.append(contentsOf: runtimes.enumerated().map { offset, runtime in
            RuntimeEvent(
                sequence: UInt64(offset + 2),
                kind: "session-changed",
                detail: "stopped",
                sessionId: runtime.sessionID,
                responseJson: nil
            )
        })
        return RuntimeObservationFrame(
            snapshot: .snapshot(snapshot: snapshot),
            catalog: profile.catalogSnapshotForTesting,
            events: events,
            oldestAvailableEvent: 1,
            newestAvailableEvent: UInt64(events.count),
            eventCursorWasStale: false,
            lostBeforeBatch: 0
        )
    }

    private func assertEveryLaunchSlotAvailable(
        _ profile: NativeRuntimeProfile,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        for _ in 0 ..< NativeRuntimeLibraryLimits.maximumSessions {
            XCTAssertEqual(
                profile.reserveSessionLaunch(),
                .admitted,
                file: file,
                line: line
            )
        }
        XCTAssertEqual(
            profile.reserveSessionLaunch(),
            .capacity(maximum: NativeRuntimeLibraryLimits.maximumSessions),
            file: file,
            line: line
        )
        for _ in 0 ..< NativeRuntimeLibraryLimits.maximumSessions {
            profile.releaseSessionLaunch()
        }
    }
}

private final class CapacityRetirementResponses: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [Data] = []

    var values: [Data] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func append(_ value: Data) {
        lock.lock()
        storage.append(value)
        lock.unlock()
    }
}
