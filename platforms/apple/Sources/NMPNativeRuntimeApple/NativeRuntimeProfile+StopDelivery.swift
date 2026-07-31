import NMPNativeRuntime

// MARK: - Stop terminal-response completion

extension NativeRuntimeProfile {
    struct StoppingSession {
        let session: RustRuntimeNappletSession
        let minimumTerminalRevision: UInt64
        /// Once Rust reports an event gap in the frame that removes this
        /// session, the Stop terminal may be gone permanently. Keep the sink
        /// instead of later pretending a clean frame delivered it.
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
        eventCursorWasStale: Bool,
        lostBeforeBatch: UInt64,
        terminalEvidenceWasLost: Bool
    ) -> StopFrameDisposition {
        guard snapshotRevision >= minimumTerminalRevision,
              !snapshotRetainsSession
        else {
            return .wait
        }
        if terminalEvidenceWasLost || eventCursorWasStale || lostBeforeBatch > 0 {
            return .preserveAfterEvidenceLoss
        }
        return .complete
    }

    func completeStopsAfterDelivery(
        frame: RuntimeObservationFrame,
        snapshot: RuntimeSnapshot,
        activeSessions: [RustRuntimeNappletSession],
        stoppingSessionsAtFrameStart: [UInt64: StoppingSession]
    ) {
        let retainedSessionIDs = Set(snapshot.sessions.map(\.id))
        let stopFrames = activeSessions.compactMap { session in
            stoppingSessionsAtFrameStart[session.sessionID].map { stopping in
                (
                    session: session,
                    disposition: Self.stopFrameDisposition(
                        snapshotRevision: snapshot.revision,
                        minimumTerminalRevision: stopping.minimumTerminalRevision,
                        snapshotRetainsSession: retainedSessionIDs.contains(session.sessionID),
                        eventCursorWasStale: frame.eventCursorWasStale,
                        lostBeforeBatch: frame.lostBeforeBatch,
                        terminalEvidenceWasLost: stopping.terminalEvidenceWasLost
                    )
                )
            }
        }
        preserveStopsWithLostEvidence(stopFrames)
        completeStopsWithDeliveredEvidence(stopFrames)
    }

    private func preserveStopsWithLostEvidence(
        _ stopFrames: [(session: RustRuntimeNappletSession, disposition: StopFrameDisposition)]
    ) {
        let evidenceLost = stopFrames.filter {
            $0.disposition == .preserveAfterEvidenceLoss
        }
        guard !evidenceLost.isEmpty else { return }
        lock.lock()
        for candidate in evidenceLost {
            let session = candidate.session
            if var stopping = stoppingSessions[session.sessionID],
               stopping.session === session
            {
                stopping.terminalEvidenceWasLost = true
                stoppingSessions[session.sessionID] = stopping
            }
        }
        lock.unlock()
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
