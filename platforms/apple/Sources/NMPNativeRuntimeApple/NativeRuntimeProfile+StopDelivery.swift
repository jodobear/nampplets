import NMPNativeRuntime

// MARK: - Stop terminal-response completion

extension NativeRuntimeProfile {
    enum StopTerminalEvidence: Equatable {
        case pending
        case delivered
        /// Rust reported that events were evicted before native delivery.
        /// The count is evidence of loss, never a claim that the original
        /// correlation-bearing responses reached their sink.
        case deliveryLost(lostBeforeBatch: UInt64)
    }

    struct StoppingSession {
        let session: RustRuntimeNappletSession
        let minimumTerminalRevision: UInt64
        /// Rust emits this session's final stopped event after every terminal
        /// response. Completion waits for that delivery watermark or explicit
        /// Rust event-loss evidence plus an accepted absent-session snapshot.
        var terminalEvidence: StopTerminalEvidence = .pending
    }

    enum StopFrameDisposition: Equatable {
        case wait
        case completeDelivered
        case completeDeliveryLost(lostBeforeBatch: UInt64)
        case completeRuntimeClosed
    }

    static func stopFrameDisposition(
        snapshotRevision: UInt64,
        minimumTerminalRevision: UInt64,
        snapshotClosed: Bool,
        snapshotRetainsSession: Bool,
        terminalEvidence: StopTerminalEvidence
    ) -> StopFrameDisposition {
        guard snapshotRevision >= minimumTerminalRevision else {
            return .wait
        }
        if snapshotClosed {
            return .completeRuntimeClosed
        }
        guard !snapshotRetainsSession else { return .wait }
        switch terminalEvidence {
        case .pending:
            return .wait
        case .delivered:
            return .completeDelivered
        case let .deliveryLost(lostBeforeBatch):
            return .completeDeliveryLost(lostBeforeBatch: lostBeforeBatch)
        }
    }

    /// Called only after every event in the frame has been handed to session
    /// sinks. Rust projects this typed stopped event after all terminal
    /// responses for the same Stop, so a loss-free batch makes it an exact
    /// delivery watermark. A stale batch can retain the marker after evicting
    /// an earlier terminal response.
    func recordStopTerminalEvidence(_ frame: RuntimeObservationFrame) {
        let lossWasReported = frame.eventCursorWasStale
            || frame.lostBeforeBatch > 0
        let terminalSessionIDs: Set<UInt64> = lossWasReported
            ? []
            : Set(
                frame.events.compactMap { event in
                    event.kind == "session-changed"
                        && event.detail == "stopped"
                        ? event.sessionId
                        : nil
                }
        )
        guard lossWasReported || !terminalSessionIDs.isEmpty else { return }
        lock.lock()
        for sessionID in Array(stoppingSessions.keys) {
            guard var stopping = stoppingSessions[sessionID] else { continue }
            if terminalSessionIDs.contains(sessionID) {
                stopping.terminalEvidence = .delivered
            } else if lossWasReported,
                      stopping.terminalEvidence == .pending
            {
                stopping.terminalEvidence = .deliveryLost(
                    lostBeforeBatch: frame.lostBeforeBatch
                )
            } else {
                continue
            }
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
                    snapshotClosed: snapshot.closed,
                    snapshotRetainsSession: retainedSessionIDs.contains(
                        stopping.session.sessionID
                    ),
                    terminalEvidence: stopping.terminalEvidence
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
            candidate.disposition != .wait
                && candidate.session.completeStopAfterTerminalEvidence()
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
