import NMPNativeRuntime

// MARK: - Stop terminal-response completion

extension NativeRuntimeProfile {
    struct StoppingSession {
        let session: RustRuntimeNappletSession
        let minimumTerminalRevision: UInt64
        /// Rust emits this session's final stopped event after every terminal
        /// response. Completion waits until that ordered marker was delivered.
        var terminalBatchDelivered = false
    }

    enum StopFrameDisposition: Equatable {
        case wait
        case complete
    }

    static func stopFrameDisposition(
        snapshotRevision: UInt64,
        minimumTerminalRevision: UInt64,
        snapshotRetainsSession: Bool,
        terminalBatchDelivered: Bool
    ) -> StopFrameDisposition {
        guard snapshotRevision >= minimumTerminalRevision,
              !snapshotRetainsSession,
              terminalBatchDelivered
        else {
            return .wait
        }
        return .complete
    }

    /// Called only after every event in the frame has been handed to session
    /// sinks. Rust projects this typed stopped event after all terminal
    /// responses for the same Stop, so a loss-free batch makes it an exact
    /// delivery watermark. A stale batch can retain the marker after evicting
    /// an earlier terminal response.
    func recordDeliveredStopTerminalEvents(_ frame: RuntimeObservationFrame) {
        guard !frame.eventCursorWasStale, frame.lostBeforeBatch == 0 else {
            return
        }
        let terminalSessionIDs = Set(
            frame.events.compactMap { event in
                event.kind == "session-changed" && event.detail == "stopped"
                    ? event.sessionId
                    : nil
            }
        )
        guard !terminalSessionIDs.isEmpty else { return }
        lock.lock()
        for sessionID in terminalSessionIDs {
            guard var stopping = stoppingSessions[sessionID] else { continue }
            stopping.terminalBatchDelivered = true
            stoppingSessions[sessionID] = stopping
        }
        lock.unlock()
    }

    func completeStopsAfterDelivery(snapshot: RuntimeSnapshot) {
        let retainedSessionIDs = Set(snapshot.sessions.map(\.id))
        lock.lock()
        let stopFrames = stoppingSessions.values.map { stopping in
            (
                session: stopping.session,
                disposition: Self.stopFrameDisposition(
                    snapshotRevision: snapshot.revision,
                    minimumTerminalRevision: stopping.minimumTerminalRevision,
                    snapshotRetainsSession: retainedSessionIDs.contains(
                        stopping.session.sessionID
                    ),
                    terminalBatchDelivered: stopping.terminalBatchDelivered
                )
            )
        }
        lock.unlock()
        completeStopsWithDeliveredEvidence(stopFrames)
    }

    private func completeStopsWithDeliveredEvidence(
        _ stopFrames: [(session: RustRuntimeNappletSession, disposition: StopFrameDisposition)]
    ) {
        let completedStops = stopFrames.compactMap { candidate in
            candidate.disposition == .complete
                && candidate.session.completeStopAfterTerminalDelivery()
                ? candidate.session
                : nil
        }
        guard !completedStops.isEmpty else { return }
        lock.lock()
        for session in completedStops {
            if sessions[session.sessionID]?.value === session {
                sessions.removeValue(forKey: session.sessionID)
            }
            if stoppingSessions[session.sessionID]?.session === session {
                stoppingSessions.removeValue(forKey: session.sessionID)
            }
        }
        lock.unlock()
    }
}
