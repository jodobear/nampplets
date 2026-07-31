import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

// MARK: - Stop terminal-response handoff

final class RuntimeSessionStopDeliveryTests: RuntimeNappletSessionTestCase {
    func testRefusedStaleFramePreservesStopAcrossLaterCleanSnapshot() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-stop-evidence-loss-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }
        let session = RustRuntimeNappletSession(
            profile: profile,
            sessionID: UInt64.max,
            maximumReadBytes: 1_024
        )
        var terminalSnapshot = try profile.snapshotForTesting
        terminalSnapshot.revision += 1
        let stopping = NativeRuntimeProfile.StoppingSession(
            session: session,
            minimumTerminalRevision: terminalSnapshot.revision
        )
        profile.lock.lock()
        profile.stoppingSessions[session.sessionID] = stopping
        profile.lock.unlock()
        profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .refused(
                    revision: terminalSnapshot.revision,
                    closed: false,
                    refusal: RuntimeRefusal(
                        code: "snapshot-projection-refused",
                        detail: "deterministic stop-loss regression",
                        occurredAtMillis: 1
                    )
                ),
                catalog: profile.catalogSnapshotForTesting,
                events: [],
                oldestAvailableEvent: 1,
                newestAvailableEvent: 1,
                eventCursorWasStale: true,
                lostBeforeBatch: 1
            )
        )
        profile.lock.lock()
        let preserved = profile.stoppingSessions[session.sessionID]
        profile.lock.unlock()
        XCTAssertEqual(preserved?.terminalEvidenceWasLost, true)

        profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .snapshot(terminalSnapshot),
                catalog: profile.catalogSnapshotForTesting,
                events: [],
                oldestAvailableEvent: 1,
                newestAvailableEvent: 1,
                eventCursorWasStale: false,
                lostBeforeBatch: 0
            )
        )
        profile.lock.lock()
        let stillPreserved = profile.stoppingSessions[session.sessionID]
        profile.lock.unlock()
        XCTAssertEqual(stillPreserved?.terminalEvidenceWasLost, true)

        XCTAssertEqual(
            NativeRuntimeProfile.stopFrameDisposition(
                snapshotRevision: 12,
                minimumTerminalRevision: 12,
                snapshotRetainsSession: false,
                terminalEvidenceWasLost: true
            ),
            .preserveAfterEvidenceLoss
        )
    }

    func testLaunchOccupancyUnionsActiveStoppingAndReservations() {
        XCTAssertEqual(
            NativeRuntimeProfile.sessionLaunchOccupancy(
                activeSessionIDs: Set((1 ... 10).map(UInt64.init)),
                stoppingSessionIDs: Set((8 ... 15).map(UInt64.init)),
                reservations: 1
            ),
            NativeRuntimeLibraryLimits.maximumSessions
        )
    }

    func testLaunchReservationCapacityIsFiniteAndObservable() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-launch-admission-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }
        let fixture = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let installed = try profile.installSignedNamed(
            title: "Launch Capacity Probe",
            eventJSON: Data(contentsOf: fixture.appendingPathComponent("event.json")),
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [
                indexDigest: Data(contentsOf: fixture.appendingPathComponent("index.html")),
            ]
        )

        for _ in 0 ..< NativeRuntimeLibraryLimits.maximumSessions {
            XCTAssertEqual(profile.reserveSessionLaunch(), .admitted)
        }
        XCTAssertEqual(
            profile.reserveSessionLaunch(),
            .capacity(maximum: NativeRuntimeLibraryLimits.maximumSessions)
        )
        XCTAssertThrowsError(try profile.launchInstalled(installed)) { error in
            XCTAssertEqual(
                error as? RuntimeNappletOpenError,
                .launchRefused(
                    detail: "Native terminal-response session capacity "
                        + "\(NativeRuntimeLibraryLimits.maximumSessions) is full"
                )
            )
        }
        for _ in 0 ..< NativeRuntimeLibraryLimits.maximumSessions {
            profile.releaseSessionLaunch()
        }
    }

    @MainActor
    func testStopDeliversPendingPublishRefusalBeforeInvalidatingSink() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-stop-delivery-\(UUID().uuidString)",
                isDirectory: true
            )
        defer { try? FileManager.default.removeItem(at: root) }
        let fixture = repositoryRoot().appendingPathComponent(
            "conformance/napplet-corpus/published/good-morning",
            isDirectory: true
        )
        let event = try Data(contentsOf: fixture.appendingPathComponent("event.json"))
        let index = try Data(contentsOf: fixture.appendingPathComponent("index.html"))
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        defer { profile.close() }

        let registration = profile.registerLocalAccount(
            secretKey: String(format: "%064x", 37)
        )
        let account = try XCTUnwrap(registration.handle)
        XCTAssertTrue(registration.accepted)
        XCTAssertTrue(profile.activateLocalAccount(handle: account).accepted)

        let artifact = try profile.openSignedNamed(
            title: "Good Morning Stop Delivery",
            eventJSON: event,
            author: author,
            dTag: "good-morning",
            blobsBySHA256: [indexDigest: index],
            grantDomains: requiredGoodMorningDomains
        )
        let runtime = try XCTUnwrap(artifact.runtimeSession)
        let sessionID = runtime.sessionID
        defer { runtime.stop() }

        let pendingAppeared = expectation(description: "pending publish appeared")
        let pendingSignaled = StopDeliveryFlag()
        let pending = try profile.observePendingWrites { update in
            let projection: NativeRuntimePendingWriteProjection
            switch update {
            case let .authoritative(value), let .next(value, _, _):
                projection = value
            }
            if projection.writes.contains(where: { $0.sessionID == sessionID }),
               pendingSignaled.setIfFalse()
            {
                pendingAppeared.fulfill()
            }
        }
        defer { pending.cancel() }

        let refusalDelivered = expectation(description: "Stop refusal reached response sink")
        let response = StopDeliveryResponse()
        runtime.setResponseSink { bytes in
            guard let envelope = try? JSONSerialization.jsonObject(with: bytes)
                    as? [String: Any],
                  envelope["type"] as? String == "outbox.publish.result",
                  envelope["id"] as? String == "stop-publish-1",
                  response.record(envelope)
            else {
                return
            }
            refusalDelivered.fulfill()
        }

        runtime.mappedEnvelope(Data(#"{"type":"shell.ready"}"#.utf8))
        runtime.mappedEnvelope(
            Data(
                #"{"type":"outbox.publish","id":"stop-publish-1","event":{"kind":1,"content":"stop terminal delivery","tags":[],"created_at":1700000000}}"#.utf8
            )
        )
        await fulfillment(of: [pendingAppeared], timeout: 10)

        runtime.stop()

        await fulfillment(of: [refusalDelivered], timeout: 10)
        XCTAssertEqual(response.ok, false)
        XCTAssertEqual(response.error, "provider operation cancelled")
        XCTAssertFalse(
            try profile.snapshotForTesting.sessions.contains {
                $0.id == sessionID
            }
        )
    }
}

private final class StopDeliveryFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    func setIfFalse() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !value else { return false }
        value = true
        return true
    }
}

private final class StopDeliveryResponse: @unchecked Sendable {
    private let lock = NSLock()
    private var recorded = false
    private var storedOK: Bool?
    private var storedError: String?

    var ok: Bool? {
        lock.lock()
        defer { lock.unlock() }
        return storedOK
    }

    var error: String? {
        lock.lock()
        defer { lock.unlock() }
        return storedError
    }

    func record(_ envelope: [String: Any]) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !recorded else { return false }
        recorded = true
        storedOK = envelope["ok"] as? Bool
        storedError = envelope["error"] as? String
        return true
    }
}
