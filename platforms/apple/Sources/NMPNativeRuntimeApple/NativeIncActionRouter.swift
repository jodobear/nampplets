import Foundation
import NMPNativeRuntime

public enum NativeWorkbenchActionKind: String, Sendable, Equatable {
    case noteOpen = "note-open"
    case profileOpen = "profile-open"
    case composeOpen = "compose-open"
}

/// One validated NAP-INC action projected from Rust. The payload remains the
/// exact bounded JSON accepted by the provider; native product code decides
/// how that action changes navigation or composer state.
public struct NativeWorkbenchAction: Sendable, Equatable {
    public let manifestAuthor: String
    public let dTag: String
    public let aggregateHash: String
    public let sessionID: UInt64
    public let sourceWindowID: UInt64
    public let kind: NativeWorkbenchActionKind
    public let payloadJSON: String

    public init(
        manifestAuthor: String,
        dTag: String,
        aggregateHash: String,
        sessionID: UInt64,
        sourceWindowID: UInt64,
        kind: NativeWorkbenchActionKind,
        payloadJSON: String
    ) {
        self.manifestAuthor = manifestAuthor
        self.dTag = dTag
        self.aggregateHash = aggregateHash
        self.sessionID = sessionID
        self.sourceWindowID = sourceWindowID
        self.kind = kind
        self.payloadJSON = payloadJSON
    }
}

public typealias NativeWorkbenchActionHandler =
    @Sendable (NativeWorkbenchAction) -> Void

/// Finite, nonblocking handoff from the Rust provider callback to AppKit's
/// main queue. A missing handler is closed, never a successful silent drop.
final class MacOSIncActionExecutor:
    NativeIncActionExecutor,
    @unchecked Sendable
{
    private struct Pending {
        let sessionID: UInt64
        let sourceWindowID: UInt64
        let item: DispatchWorkItem
    }

    private static let maximumPendingActions = 64
    private static let maximumPayloadBytes = 64 * 1_024

    private let lock = NSLock()
    private var nextID: UInt64 = 0
    private var pending: [UInt64: Pending] = [:]
    private var handler: NativeWorkbenchActionHandler?
    private var isClosed = false

    func setHandler(_ handler: NativeWorkbenchActionHandler?) {
        lock.lock()
        self.handler = handler
        if handler == nil {
            let pending = self.pending.values.map(\.item)
            self.pending.removeAll(keepingCapacity: false)
            lock.unlock()
            pending.forEach { $0.cancel() }
        } else {
            lock.unlock()
        }
    }

    func tryEnqueue(
        request: NativeIncActionRequest
    ) -> NativeIncActionEnqueueResult {
        guard request.payloadJson.utf8.count <= Self.maximumPayloadBytes,
              let kind = NativeWorkbenchActionKind(rawValue: request.kind)
        else {
            return .closed
        }
        let action = NativeWorkbenchAction(
            manifestAuthor: request.manifestAuthor,
            dTag: request.dTag,
            aggregateHash: request.aggregateHash,
            sessionID: request.sessionId,
            sourceWindowID: request.sourceWindowId,
            kind: kind,
            payloadJSON: request.payloadJson
        )

        lock.lock()
        guard !isClosed, handler != nil else {
            lock.unlock()
            return .closed
        }
        let increment = nextID.addingReportingOverflow(1)
        guard pending.count < Self.maximumPendingActions,
              !increment.overflow
        else {
            lock.unlock()
            return .backpressure
        }
        let id = increment.partialValue
        nextID = id
        let item = DispatchWorkItem { [weak self] in
            self?.deliver(id: id, action: action)
        }
        pending[id] = Pending(
            sessionID: request.sessionId,
            sourceWindowID: request.sourceWindowId,
            item: item
        )
        lock.unlock()
        DispatchQueue.main.async(execute: item)
        return .accepted
    }

    func sessionEnded(end: NativeIncActionEnd) {
        lock.lock()
        let matching = pending.filter { _, pending in
            pending.sessionID == end.sessionId
                && pending.sourceWindowID == end.sourceWindowId
        }
        for id in matching.keys {
            pending.removeValue(forKey: id)
        }
        lock.unlock()
        matching.values.forEach { $0.item.cancel() }
    }

    func close() {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        isClosed = true
        handler = nil
        let pending = pending.values.map(\.item)
        self.pending.removeAll(keepingCapacity: false)
        lock.unlock()
        pending.forEach { $0.cancel() }
    }

    private func deliver(id: UInt64, action: NativeWorkbenchAction) {
        lock.lock()
        guard let pending = pending.removeValue(forKey: id),
              !pending.item.isCancelled,
              !isClosed,
              let handler
        else {
            lock.unlock()
            return
        }
        lock.unlock()
        handler(action)
    }
}
