import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

// MARK: - Terminal batches queued before wrapper Stop

final class RuntimeSessionQueuedTerminalTests: RuntimeNappletSessionTestCase {
    func testRustStopThenWrapperStopPreservesQueuedTerminalResponse() throws {
        try assertQueuedTerminalDelivery(
            name: "queued-stopped-terminal",
            detail: "stopped",
            markerBeforeWrapperStop: false
        ) { profile, sessionID in
            profile.controller.stop(sessionId: sessionID)
        }
    }

    func testRustCrashMarkerDeliveredBeforeWrapperStopStillRetiresSink() throws {
        try assertQueuedTerminalDelivery(
            name: "queued-crashed-terminal",
            detail: "crashed",
            markerBeforeWrapperStop: true
        ) { profile, sessionID in
            profile.controller.crash(
                sessionId: sessionID,
                reason: "deterministic queued crash"
            )
        }
    }

    private func assertQueuedTerminalDelivery(
        name: String,
        detail: String,
        markerBeforeWrapperStop: Bool,
        endInRust: (NativeRuntimeProfile, UInt64) -> Void
    ) throws {
        let fixture = try makeQueuedFixture(name)
        defer { fixture.profile.close() }
        let responses = QueuedTerminalResponses()
        fixture.runtime.setResponseSink(responses.append)
        let live = try fixture.profile.snapshotForTesting
        let sessionID = fixture.runtime.sessionID

        endInRust(fixture.profile, sessionID)
        let ended = try NativeRuntimeProfile.initialSnapshot(
            from: fixture.profile.controller.snapshot()
        )
        XCTAssertFalse(ended.sessions.contains { $0.id == sessionID })
        let responseJSON =
            #"{"type":"outbox.publish.result","id":"queued-terminal","ok":false,"error":"provider operation cancelled"}"#
        let terminalFrame = queuedTerminalFrame(
            profile: fixture.profile,
            snapshot: live,
            sessionID: sessionID,
            detail: detail,
            responseJSON: responseJSON
        )

        if markerBeforeWrapperStop {
            fixture.profile.update(frame: terminalFrame)
        }
        fixture.runtime.stop()
        XCTAssertEqual(try fixture.profile.snapshotForTesting.revision, ended.revision)
        if markerBeforeWrapperStop {
            let duplicateStopWindow = expectation(
                description: "duplicate Stop command had time to drain"
            )
            DispatchQueue.global().asyncAfter(deadline: .now() + 1) {
                duplicateStopWindow.fulfill()
            }
            wait(for: [duplicateStopWindow], timeout: 2)
            let settled = try fixture.profile.snapshotForTesting
            XCTAssertEqual(settled.revision, ended.revision)
            XCTAssertEqual(
                settled.recentErrors.map(\.code),
                ended.recentErrors.map(\.code)
            )
            XCTAssertEqual(settled.droppedErrors, ended.droppedErrors)
            XCTAssertEqual(
                settled.boundaryRefusals.map(\.code),
                ended.boundaryRefusals.map(\.code)
            )
            XCTAssertEqual(
                settled.droppedBoundaryRefusals,
                ended.droppedBoundaryRefusals
            )
        }
        if !markerBeforeWrapperStop {
            assertStopping(fixture, evidence: .pending)
            fixture.profile.update(frame: terminalFrame)
            assertStopping(fixture, evidence: .delivered)
        }

        XCTAssertEqual(responses.values, [Data(responseJSON.utf8)])
        fixture.profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .snapshot(snapshot: ended),
                catalog: fixture.profile.catalogSnapshotForTesting,
                events: [],
                oldestAvailableEvent: 12,
                newestAvailableEvent: 12,
                eventCursorWasStale: false,
                lostBeforeBatch: 0
            )
        )
        assertRetired(fixture)
        assertEveryLaunchSlotAvailable(fixture.profile)
    }

    private typealias QueuedFixture = (
        profile: NativeRuntimeProfile,
        runtime: RustRuntimeNappletSession
    )

    private func makeQueuedFixture(_ name: String) throws -> QueuedFixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-\(name)-\(UUID().uuidString)",
                isDirectory: true
            )
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        try quiesceObservation(profile)
        let published = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let artifact = try profile.openSignedNamed(
            title: "Queued Terminal",
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

    private func quiesceObservation(_ profile: NativeRuntimeProfile) throws {
        let entered = expectation(description: "observer callback entered")
        let returned = expectation(description: "observer callback returned")
        let allowReturn = DispatchSemaphore(value: 0)
        let library = try profile.observeInstalledLibrary { update in
            guard case .next = update else { return }
            entered.fulfill()
            _ = allowReturn.wait(timeout: .now() + 5)
            returned.fulfill()
        }
        profile.setInstalledLibraryFilter("queued-terminal-observer-barrier")
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

    private func queuedTerminalFrame(
        profile: NativeRuntimeProfile,
        snapshot: RuntimeSnapshot,
        sessionID: UInt64,
        detail: String,
        responseJSON: String
    ) -> RuntimeObservationFrame {
        RuntimeObservationFrame(
            snapshot: .snapshot(snapshot: snapshot),
            catalog: profile.catalogSnapshotForTesting,
            events: [
                RuntimeEvent(
                    sequence: 11,
                    kind: "envelope-handled",
                    detail: "queued terminal response",
                    sessionId: sessionID,
                    responseJson: responseJSON
                ),
                RuntimeEvent(
                    sequence: 12,
                    kind: "session-changed",
                    detail: detail,
                    sessionId: sessionID,
                    responseJson: nil
                ),
            ],
            oldestAvailableEvent: 11,
            newestAvailableEvent: 12,
            eventCursorWasStale: false,
            lostBeforeBatch: 0
        )
    }

    private func assertStopping(
        _ fixture: QueuedFixture,
        evidence: NativeRuntimeProfile.StopTerminalEvidence,
        file: StaticString = #filePath,
        line: UInt = #line
    ) {
        fixture.profile.lock.lock()
        let stopping = fixture.profile.stoppingSessions[fixture.runtime.sessionID]
        fixture.profile.lock.unlock()
        XCTAssertNotNil(stopping, file: file, line: line)
        XCTAssertEqual(stopping?.terminalEvidence, evidence, file: file, line: line)
    }

    private func assertRetired(
        _ fixture: QueuedFixture,
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

private final class QueuedTerminalResponses: @unchecked Sendable {
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
