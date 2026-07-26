import Foundation
import NMPNativeRuntime

/// Identifies the NAP-INTENT handler a launched/focused window should
/// target. `Principal` (manifest author + d tag + aggregate hash) already
/// *is* an exact-build identity, matching `WorkbenchExactBuildIdentity` 1:1.
public struct NativeIntentActivationHandlerRequest: Sendable, Equatable {
    public let manifestAuthor: String
    public let dTag: String
    public let aggregateHash: String

    public init(manifestAuthor: String, dTag: String, aggregateHash: String) {
        self.manifestAuthor = manifestAuthor
        self.dTag = dTag
        self.aggregateHash = aggregateHash
    }
}

public typealias NativeIntentActivationHandler =
    @Sendable (NativeIntentActivationHandlerRequest) -> Void

/// Bridges Rust's NAP-INTENT dispatcher -- which may call from an arbitrary
/// background thread, before any webview session exists -- onto AppKit's
/// main queue. Unlike `MacOSIncActionExecutor` this is not scoped to a
/// session or window, so there is nothing to track or cancel per-session.
final class MacOSIntentActivationExecutor:
    NativeIntentActivationExecutor,
    @unchecked Sendable
{
    private let lock = NSLock()
    private var handler: NativeIntentActivationHandler?

    func setHandler(_ handler: NativeIntentActivationHandler?) {
        lock.lock()
        self.handler = handler
        lock.unlock()
    }

    func close() {
        setHandler(nil)
    }

    func focusOrLaunch(handler request: NativeIntentActivationRequest) {
        lock.lock()
        let handler = self.handler
        lock.unlock()
        guard let handler else { return }
        let mapped = NativeIntentActivationHandlerRequest(
            manifestAuthor: request.manifestAuthor,
            dTag: request.dTag,
            aggregateHash: request.aggregateHash
        )
        DispatchQueue.main.async {
            handler(mapped)
        }
    }
}
