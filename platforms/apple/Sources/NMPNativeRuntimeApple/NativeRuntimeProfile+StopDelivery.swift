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
        /// Rust emits the final stopped/crashed event after every terminal
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

    func stopSession(_ session: RustRuntimeNappletSession) {
        let sessionID = session.sessionID
        let snapshotProjection = pullSnapshotProjection()
        let terminalEvidence = session.stopTerminalEvidence()
        lock.lock()
        let isRegistered = sessions[sessionID]?.value === session
        let acceptedSnapshot: RuntimeSnapshot
        switch snapshotProjection {
        case let .snapshot(snapshot):
            acceptedSnapshot = snapshot
        case .refused:
            acceptedSnapshot = lastAcceptedSnapshot
        }
        let profileClosed = isClosed || acceptedSnapshot.closed
        let rustRetainsSession = acceptedSnapshot.sessions.contains {
            $0.id == sessionID
        }
        let rustAlreadyEnded = terminalEvidence != .pending
            || !rustRetainsSession
        let shouldStop = !profileClosed && isRegistered && !rustAlreadyEnded
        let shouldAwaitExistingTerminal = !profileClosed
            && isRegistered
            && rustAlreadyEnded
        if shouldStop {
            // Rust dispatch and observation are asynchronous. Retain the
            // borrowed session until update(frame:) hands off the terminal
            // refusal emitted by Stop.
            stoppingSessions[sessionID] = StoppingSession(
                session: session,
                minimumTerminalRevision: acceptedSnapshot.revision == UInt64.max
                    ? UInt64.max
                    : acceptedSnapshot.revision + 1,
                terminalEvidence: terminalEvidence
            )
        } else if shouldAwaitExistingTerminal {
            // Rust already ended the session, so a second Stop would produce
            // only UnknownSession. Keep the sink until its already-queued
            // stopped/crashed marker, explicit event loss, or runtime closure.
            stoppingSessions[sessionID] = StoppingSession(
                session: session,
                minimumTerminalRevision: acceptedSnapshot.revision,
                terminalEvidence: terminalEvidence
            )
        } else if isRegistered {
            sessions.removeValue(forKey: sessionID)
            stoppingSessions.removeValue(forKey: sessionID)
        }
        lock.unlock()
        if shouldStop || shouldAwaitExistingTerminal {
            // Close the race where an observer handed off the marker between
            // the evidence read above and insertion into stoppingSessions.
            recordStopTerminalEvidence()
        }

        let stopStillRequired = shouldStop
            && session.stopTerminalEvidence() == .pending
        if stopStillRequired {
            controller.stop(sessionId: sessionID)
        } else if shouldStop || shouldAwaitExistingTerminal {
            lock.lock()
            let completionSnapshot = lastAcceptedSnapshot
            lock.unlock()
            completeStopsAfterDelivery(snapshot: completionSnapshot)
        } else {
            session.profileDidClose()
        }
    }

    /// Synchronizes evidence latched by each session only after that session
    /// handed frame responses to its sink. Keeping the evidence on the session
    /// also covers a Rust Stop/Crash whose marker arrived before wrapper Stop.
    func recordStopTerminalEvidence() {
        lock.lock()
        let candidates = stoppingSessions.map { ($0.key, $0.value.session) }
        lock.unlock()
        let evidence = candidates.map { ($0.0, $0.1, $0.1.stopTerminalEvidence()) }
        lock.lock()
        for (sessionID, session, terminalEvidence) in evidence {
            guard var stopping = stoppingSessions[sessionID],
                  stopping.session === session
            else { continue }
            stopping.terminalEvidence = terminalEvidence
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
        completeStopsWithTerminalEvidence(stopFrames)
    }

    /// Rust may terminate a session while its UI keeps the native wrapper and
    /// never calls Stop. Retire that wrapper only after its terminal batch was
    /// delivered (or loss/closure was explicit), then release launch capacity.
    func retireRustTerminatedSessionsAfterDelivery(snapshot: RuntimeSnapshot) {
        let retainedSessionIDs = Set(snapshot.sessions.map(\.id))
        lock.lock()
        let candidates = sessions.compactMap { sessionID, weakSession in
            guard stoppingSessions[sessionID] == nil,
                  snapshot.closed || !retainedSessionIDs.contains(sessionID)
            else { return nil }
            return weakSession.value
        }
        lock.unlock()

        let retired = candidates.filter {
            $0.completeRustTerminationAfterDelivery(runtimeClosed: snapshot.closed)
        }
        guard !retired.isEmpty else { return }
        lock.lock()
        for session in retired where sessions[session.sessionID]?.value === session {
            sessions.removeValue(forKey: session.sessionID)
        }
        lock.unlock()
    }

    private func completeStopsWithTerminalEvidence(
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

    // MARK: Profile close and retained terminal sessions

    public func close() {
        accountLock.lock()
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            accountLock.unlock()
            return
        }
        isClosed = true
        let observation = observation
        self.observation = nil
        var activeSessionsByID = stoppingSessions.mapValues(\.session)
        for (sessionID, session) in sessions {
            if let value = session.value {
                activeSessionsByID[sessionID] = value
            }
        }
        let activeSessions = Array(activeSessionsByID.values)
        sessions.removeAll()
        stoppingSessions.removeAll()
        activityObservers.removeAll()
        libraryObservers.removeAll()
        catalogObservers.removeAll()
        pendingWriteObservers.removeAll()
        receiptObservers.removeAll()
        lock.unlock()
        // Released here, for the same reason `lock` is released above.
        //
        // `accountLock` is a plain `NSLock`. Every call below can block until
        // an in-flight runtime callback returns, and a callback that reaches
        // any account method -- `accountSnapshot()` and its siblings all take
        // this lock -- would then wait on a lock this thread is still holding.
        // Neither side can proceed. It presents as a teardown that is instant
        // whenever no callback happens to be in flight and never returns when
        // one is.
        //
        // `isClosed` is already true under `lock`, so a second `close()` still
        // returns at the guard above.
        accountLock.unlock()

        observation?.stop()
        for session in activeSessions {
            session.profileDidClose()
        }
        appearanceSource.close()
        settingsExecutor.close()
        incActionExecutor.close()
        intentActivationExecutor.close()

        // Retaken only around the controller, which is the single thing this
        // lock exists to serialise: an account call either completes before
        // the controller closes or begins after, never during. Nothing below
        // this line invokes an application callback.
        accountLock.lock()
        controller.close()
        accountLock.unlock()
    }
}
