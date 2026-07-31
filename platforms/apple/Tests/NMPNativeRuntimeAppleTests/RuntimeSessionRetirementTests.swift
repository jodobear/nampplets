import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

// MARK: - Rust-authoritative terminal retirement

final class RuntimeSessionRetirementTests: RuntimeNappletSessionTestCase {
    func testStopAfterAcceptedRustClosureIgnoresRetainedSnapshotRows() throws {
        let fixture = try makeLaunchedFixture(
            "closed-profile-before-wrapper-stop",
            automaticObservation: false
        )
        defer { fixture.profile.close() }
        var terminal = try fixture.profile.snapshotForTesting
        XCTAssertTrue(terminal.sessions.contains { $0.id == fixture.runtime.sessionID })
        terminal.revision += 1
        terminal.closed = true
        fixture.profile.update(
            frame: observationFrame(
                profile: fixture.profile,
                snapshot: terminal
            )
        )

        fixture.runtime.stop()

        assertSessionRetired(fixture)
        assertEveryLaunchSlotAvailable(fixture.profile)
    }

    func testAcceptedRustClosureRetiresAnAlreadyPendingStop() throws {
        let fixture = try makeLaunchedFixture(
            "closed-profile-during-wrapper-stop",
            automaticObservation: false
        )
        defer { fixture.profile.close() }
        var terminal = try fixture.profile.snapshotForTesting
        let responses = StopRetirementResponses()
        fixture.runtime.setResponseSink(responses.append)

        fixture.runtime.stop()
        terminal.revision += 1
        terminal.closed = true
        fixture.profile.update(
            frame: observationFrame(
                profile: fixture.profile,
                snapshot: terminal
            )
        )

        XCTAssertEqual(responses.count, 0)
        assertSessionRetired(fixture)
        assertEveryLaunchSlotAvailable(fixture.profile)
    }

    func testLossyStopBatchRetiresWithoutFabricatingAResponse() throws {
        let fixture = try makeLaunchedFixture(
            "lossy-stop-retirement",
            automaticObservation: false
        )
        defer { fixture.profile.close() }
        let responses = StopRetirementResponses()
        fixture.runtime.setResponseSink(responses.append)

        fixture.runtime.stop()
        let ended = try fixture.profile.snapshotForTesting
        XCTAssertFalse(ended.sessions.contains { $0.id == fixture.runtime.sessionID })

        fixture.profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .snapshot(snapshot: ended),
                catalog: fixture.profile.catalogSnapshotForTesting,
                events: [],
                oldestAvailableEvent: 9,
                newestAvailableEvent: 9,
                eventCursorWasStale: true,
                lostBeforeBatch: 3
            )
        )

        XCTAssertEqual(responses.count, 0)
        assertSessionRetired(fixture)
        assertEveryLaunchSlotAvailable(fixture.profile)
    }

    private typealias LaunchedFixture = (
        profile: NativeRuntimeProfile,
        runtime: RustRuntimeNappletSession
    )

    private func makeLaunchedFixture(
        _ name: String,
        automaticObservation: Bool = true
    ) throws -> LaunchedFixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-\(name)-\(UUID().uuidString)",
                isDirectory: true
            )
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        if !automaticObservation {
            try quiesceAutomaticObservation(profile)
        }
        let published = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let artifact = try profile.openSignedNamed(
            title: "Stop Retirement",
            eventJSON: Data(contentsOf: published.appendingPathComponent("event.json")),
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [
                indexDigest: Data(
                    contentsOf: published.appendingPathComponent("index.html")
                ),
            ],
            grantDomains: requiredGoodMorningDomains
        )
        return (profile, try XCTUnwrap(artifact.runtimeSession as? RustRuntimeNappletSession))
    }

    private func quiesceAutomaticObservation(
        _ profile: NativeRuntimeProfile
    ) throws {
        let callbackEntered = expectation(description: "observer callback entered")
        let callbackReturned = expectation(description: "observer callback returned")
        let allowReturn = DispatchSemaphore(value: 0)
        let libraryObservation = try profile.observeInstalledLibrary { update in
            guard case .next = update else { return }
            callbackEntered.fulfill()
            _ = allowReturn.wait(timeout: .now() + 5)
            callbackReturned.fulfill()
        }
        profile.setInstalledLibraryFilter("stop-retirement-observer-barrier")
        wait(for: [callbackEntered], timeout: 2)

        profile.lock.lock()
        let observation = profile.observation
        profile.observation = nil
        profile.lock.unlock()
        observation?.stop()
        allowReturn.signal()
        wait(for: [callbackReturned], timeout: 2)
        libraryObservation.cancel()
    }

    private func observationFrame(
        profile: NativeRuntimeProfile,
        snapshot: RuntimeSnapshot
    ) -> RuntimeObservationFrame {
        RuntimeObservationFrame(
            snapshot: .snapshot(snapshot: snapshot),
            catalog: profile.catalogSnapshotForTesting,
            events: [],
            oldestAvailableEvent: 0,
            newestAvailableEvent: 0,
            eventCursorWasStale: false,
            lostBeforeBatch: 0
        )
    }

    private func assertSessionRetired(
        _ fixture: LaunchedFixture,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        fixture.profile.lock.lock()
        let registered = fixture.profile.sessions[fixture.runtime.sessionID]
        let stopping = fixture.profile.stoppingSessions[fixture.runtime.sessionID]
        fixture.profile.lock.unlock()
        XCTAssertNil(registered, file: file, line: line)
        XCTAssertNil(stopping, file: file, line: line)
    }

    private func assertEveryLaunchSlotAvailable(
        _ profile: NativeRuntimeProfile,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        for _ in 0 ..< NativeRuntimeLibraryLimits.maximumSessions {
            XCTAssertEqual(profile.reserveSessionLaunch(), .admitted, file: file, line: line)
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

private final class StopRetirementResponses: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Data] = []

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return values.count
    }

    func append(_ value: Data) {
        lock.lock()
        values.append(value)
        lock.unlock()
    }
}
