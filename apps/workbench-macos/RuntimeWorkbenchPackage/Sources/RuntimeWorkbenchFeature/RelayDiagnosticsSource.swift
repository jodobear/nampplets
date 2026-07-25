import Foundation
import NMPNativeRuntimeApple

@MainActor
public protocol RelayDiagnosticsSubscription {
    func cancel()
}

@MainActor
public protocol RelayDiagnosticsSource {
    func subscribe(
        receive: @escaping @MainActor (InspectorRelayDiagnosticsSnapshot) -> Void
    ) -> any RelayDiagnosticsSubscription
}

/// Opens the Rust-owned NMP diagnostics observation for exactly the lifetime
/// of the returned subscription. Unlike `RuntimeWorkbenchActivitySource`,
/// there is no profile-owned always-on native stream to fan out here: each
/// `subscribe` call opens its own refcounted native observation and
/// `cancel()` withdraws real NMP relay accounting, not just a Swift closure.
@MainActor
public final class RuntimeWorkbenchRelayDiagnosticsSource: RelayDiagnosticsSource {
    private let profile: WorkbenchRuntimeProfile

    public init(profile: WorkbenchRuntimeProfile) {
        self.profile = profile
    }

    public func subscribe(
        receive: @escaping @MainActor (InspectorRelayDiagnosticsSnapshot) -> Void
    ) -> any RelayDiagnosticsSubscription {
        let mailbox = RelayDiagnosticsUpdateMailbox()
        mailbox.bind { snapshot in
            receive(InspectorRelayDiagnosticsSnapshot(snapshot))
        }
        do {
            let observation = try profile.native.observeRelayDiagnostics {
                [mailbox] snapshot in
                mailbox.offer(snapshot)
            }
            return RuntimeWorkbenchRelayDiagnosticsSubscription(
                mailbox: mailbox,
                cancellation: { observation.cancel() }
            )
        } catch {
            receive(.notObserving)
            return RuntimeWorkbenchRelayDiagnosticsSubscription(
                mailbox: mailbox,
                cancellation: {}
            )
        }
    }
}

@MainActor
private final class RuntimeWorkbenchRelayDiagnosticsSubscription:
    RelayDiagnosticsSubscription
{
    private let mailbox: RelayDiagnosticsUpdateMailbox
    private var cancellation: (@Sendable () -> Void)?

    init(mailbox: RelayDiagnosticsUpdateMailbox, cancellation: @escaping @Sendable () -> Void) {
        self.mailbox = mailbox
        self.cancellation = cancellation
    }

    func cancel() {
        mailbox.close()
        let cancellation = cancellation
        self.cancellation = nil
        cancellation?()
    }

    deinit {
        mailbox.close()
        let cancellation = cancellation
        DispatchQueue.main.async {
            cancellation?()
        }
    }
}

/// One-slot replacement mailbox: at most one main-queue delivery is pending.
/// Relay diagnostics can update once per received event, far faster than the
/// UI needs to render — coalescing prevents an unbounded backlog when the
/// runtime produces updates faster than SwiftUI consumes them.
private final class RelayDiagnosticsUpdateMailbox: @unchecked Sendable {
    typealias Handler = @MainActor @Sendable (NativeRuntimeRelayDiagnosticsSnapshot) -> Void

    private let lock = NSLock()
    private var handler: Handler?
    private var pending: NativeRuntimeRelayDiagnosticsSnapshot?
    private var isScheduled = false
    private var isClosed = false

    @MainActor
    func bind(_ handler: @escaping Handler) {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        self.handler = handler
        let shouldSchedule = pending != nil && !isScheduled
        if shouldSchedule {
            isScheduled = true
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    func offer(_ snapshot: NativeRuntimeRelayDiagnosticsSnapshot) {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        pending = snapshot
        let shouldSchedule = handler != nil && !isScheduled
        if shouldSchedule {
            isScheduled = true
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    func close() {
        lock.lock()
        isClosed = true
        pending = nil
        handler = nil
        lock.unlock()
    }

    private func scheduleDrain() {
        DispatchQueue.main.async { [weak self] in
            self?.drainOnMainQueue()
        }
    }

    private func drainOnMainQueue() {
        lock.lock()
        guard !isClosed, let snapshot = pending, let handler else {
            isScheduled = false
            lock.unlock()
            return
        }
        pending = nil
        lock.unlock()

        MainActor.assumeIsolated {
            handler(snapshot)
        }

        lock.lock()
        let shouldSchedule = !isClosed && pending != nil && self.handler != nil
        if !shouldSchedule {
            isScheduled = false
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    deinit {
        close()
    }
}
