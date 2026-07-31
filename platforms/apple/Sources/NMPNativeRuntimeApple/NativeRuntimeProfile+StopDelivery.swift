import NMPNativeRuntime

// MARK: - Stop terminal-response completion

extension NativeRuntimeProfile {
    struct StoppingSession {
        let session: RustRuntimeNappletSession
        let minimumTerminalRevision: UInt64
        /// Any event gap while this Stop is pending can contain its terminal
        /// response. Keep the sink instead of treating a later clean snapshot
        /// as proof that the response was delivered.
        var terminalEvidenceWasLost = false
    }

    enum StopFrameDisposition: Equatable {
        case wait
        case complete
        case preserveAfterEvidenceLoss
    }

    static func stopFrameDisposition(
        snapshotRevision: UInt64,
        minimumTerminalRevision: UInt64,
        snapshotRetainsSession: Bool,
        terminalEvidenceWasLost: Bool
    ) -> StopFrameDisposition {
        guard snapshotRevision >= minimumTerminalRevision,
              !snapshotRetainsSession
        else {
            return .wait
        }
        if terminalEvidenceWasLost {
            return .preserveAfterEvidenceLoss
        }
        return .complete
    }

    /// Called while the profile lock is held, before snapshot projection can
    /// refuse the frame. Event replay has already advanced on the Rust side, so
    /// every pending Stop must remember the gap even without a usable snapshot.
    func latchStopEventLossLocked(_ frame: RuntimeObservationFrame) {
        guard frame.eventCursorWasStale || frame.lostBeforeBatch > 0 else {
            return
        }
        for sessionID in Array(stoppingSessions.keys) {
            if var stopping = stoppingSessions[sessionID] {
                stopping.terminalEvidenceWasLost = true
                stoppingSessions[sessionID] = stopping
            }
        }
    }

    func completeStopsAfterDelivery(
        snapshot: RuntimeSnapshot,
        stoppingSessionsAtFrameStart: [UInt64: StoppingSession]
    ) {
        let retainedSessionIDs = Set(snapshot.sessions.map(\.id))
        let stopFrames = stoppingSessionsAtFrameStart.values.map { stopping in
            (
                session: stopping.session,
                disposition: Self.stopFrameDisposition(
                    snapshotRevision: snapshot.revision,
                    minimumTerminalRevision: stopping.minimumTerminalRevision,
                    snapshotRetainsSession: retainedSessionIDs.contains(
                        stopping.session.sessionID
                    ),
                    terminalEvidenceWasLost: stopping.terminalEvidenceWasLost
                )
            )
        }
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
