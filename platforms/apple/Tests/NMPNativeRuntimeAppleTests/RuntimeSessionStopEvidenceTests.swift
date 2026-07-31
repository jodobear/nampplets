import Foundation
import NMPNativeRuntime
import XCTest
@testable import NMPNativeRuntimeApple

// MARK: - Stop terminal-event evidence

final class RuntimeSessionStopEvidenceTests: RuntimeNappletSessionTestCase {
    func testPreStopStaleFrameCannotPoisonLaterTerminalDelivery() throws {
        let fixture = try makeStoppingFixture("pre-stop-stale")
        defer { fixture.profile.close() }

        fixture.profile.lock.lock()
        let stopping = try XCTUnwrap(
            fixture.profile.stoppingSessions.removeValue(
                forKey: fixture.session.sessionID
            )
        )
        fixture.profile.lock.unlock()
        let frameCapturedBeforeStop = refusedStaleFrame(
            profile: fixture.profile,
            revision: fixture.snapshot.revision
        )
        fixture.profile.lock.lock()
        fixture.profile.stoppingSessions[fixture.session.sessionID] = stopping
        fixture.profile.lock.unlock()

        fixture.profile.update(frame: frameCapturedBeforeStop)
        XCTAssertEqual(
            stoppingSession(in: fixture.profile, id: fixture.session.sessionID)?
                .terminalEvidence,
            .pending
        )

        fixture.profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .snapshot(snapshot: fixture.snapshot),
                catalog: fixture.profile.catalogSnapshotForTesting,
                events: [
                    RuntimeEvent(
                        sequence: 2,
                        kind: "session-changed",
                        detail: "stopped",
                        sessionId: fixture.session.sessionID,
                        responseJson: nil
                    ),
                ],
                oldestAvailableEvent: 2,
                newestAvailableEvent: 2,
                eventCursorWasStale: false,
                lostBeforeBatch: 0
            )
        )
        XCTAssertEqual(
            stoppingSession(in: fixture.profile, id: fixture.session.sessionID)?
                .terminalEvidence,
            .delivered
        )
        XCTAssertEqual(
            NativeRuntimeProfile.stopFrameDisposition(
                snapshotRevision: fixture.snapshot.revision,
                minimumTerminalRevision: fixture.snapshot.revision,
                snapshotClosed: false,
                snapshotRetainsSession: false,
                terminalEvidence: .delivered
            ),
            .completeDelivered
        )
    }

    func testLostPostStopBatchPreservesSinkAcrossLaterCleanSnapshot() throws {
        let fixture = try makeStoppingFixture("post-stop-loss")
        defer { fixture.profile.close() }

        fixture.profile.update(
            frame: refusedStaleFrame(
                profile: fixture.profile,
                revision: fixture.snapshot.revision
            )
        )
        fixture.profile.update(
            frame: RuntimeObservationFrame(
                snapshot: .snapshot(snapshot: fixture.snapshot),
                catalog: fixture.profile.catalogSnapshotForTesting,
                events: [],
                oldestAvailableEvent: 2,
                newestAvailableEvent: 2,
                eventCursorWasStale: false,
                lostBeforeBatch: 0
            )
        )
        XCTAssertEqual(
            stoppingSession(in: fixture.profile, id: fixture.session.sessionID)?
                .terminalEvidence,
            .deliveryLost(lostBeforeBatch: 1)
        )
        XCTAssertEqual(
            NativeRuntimeProfile.stopFrameDisposition(
                snapshotRevision: fixture.snapshot.revision,
                minimumTerminalRevision: fixture.snapshot.revision,
                snapshotClosed: false,
                snapshotRetainsSession: false,
                terminalEvidence: .deliveryLost(lostBeforeBatch: 1)
            ),
            .completeDeliveryLost(lostBeforeBatch: 1)
        )
    }

    func testStaleBatchContainingTerminalMarkerDoesNotProveDelivery() throws {
        let fixture = try makeStoppingFixture("stale-terminal-marker")
        defer { fixture.profile.close() }

        let lossIndicators: [(cursorWasStale: Bool, lostBeforeBatch: UInt64)] = [
            (true, 1),
            (true, 0),
            (false, 1),
        ]
        for loss in lossIndicators {
            fixture.profile.update(
                frame: RuntimeObservationFrame(
                    snapshot: .snapshot(snapshot: fixture.snapshot),
                    catalog: fixture.profile.catalogSnapshotForTesting,
                    events: [
                        RuntimeEvent(
                            sequence: 2,
                            kind: "session-changed",
                            detail: "stopped",
                            sessionId: fixture.session.sessionID,
                            responseJson: nil
                        ),
                    ],
                    oldestAvailableEvent: 2,
                    newestAvailableEvent: 2,
                    eventCursorWasStale: loss.cursorWasStale,
                    lostBeforeBatch: loss.lostBeforeBatch
                )
            )
            XCTAssertEqual(
                stoppingSession(in: fixture.profile, id: fixture.session.sessionID)?
                    .terminalEvidence,
                .deliveryLost(lostBeforeBatch: 1)
            )
        }
        XCTAssertEqual(
            NativeRuntimeProfile.stopFrameDisposition(
                snapshotRevision: fixture.snapshot.revision,
                minimumTerminalRevision: fixture.snapshot.revision,
                snapshotClosed: false,
                snapshotRetainsSession: false,
                terminalEvidence: .deliveryLost(lostBeforeBatch: 1)
            ),
            .completeDeliveryLost(lostBeforeBatch: 1)
        )
    }

    private func makeStoppingFixture(
        _ name: String
    ) throws -> (
        profile: NativeRuntimeProfile,
        session: RustRuntimeNappletSession,
        snapshot: RuntimeSnapshot
    ) {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "runtime-apple-\(name)-\(UUID().uuidString)",
                isDirectory: true
            )
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        let profile = try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(storageRoot: root)
        )
        let session = RustRuntimeNappletSession(
            profile: profile,
            sessionID: UInt64.max,
            maximumReadBytes: 1_024
        )
        var snapshot = try profile.snapshotForTesting
        snapshot.revision += 1
        profile.lock.lock()
        profile.stoppingSessions[session.sessionID] = .init(
            session: session,
            minimumTerminalRevision: snapshot.revision
        )
        profile.lock.unlock()
        return (profile, session, snapshot)
    }

    private func refusedStaleFrame(
        profile: NativeRuntimeProfile,
        revision: UInt64
    ) -> RuntimeObservationFrame {
        RuntimeObservationFrame(
            snapshot: .refused(
                revision: revision,
                closed: false,
                refusal: RuntimeRefusal(
                    code: "snapshot-projection-refused",
                    detail: "deterministic stop-loss regression",
                    occurredAtMillis: 1
                )
            ),
            catalog: profile.catalogSnapshotForTesting,
            events: [],
            oldestAvailableEvent: 2,
            newestAvailableEvent: 2,
            eventCursorWasStale: true,
            lostBeforeBatch: 1
        )
    }

    private func stoppingSession(
        in profile: NativeRuntimeProfile,
        id: UInt64
    ) -> NativeRuntimeProfile.StoppingSession? {
        profile.lock.lock()
        defer { profile.lock.unlock() }
        return profile.stoppingSessions[id]
    }
}
