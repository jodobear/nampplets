import Foundation

/// A runtime-owned account identifier.
///
/// The value is intentionally opaque to the Workbench. It is passed back only
/// to the account manager and must never be shown as account identity.
public struct WorkbenchAccountHandle: Hashable, Sendable {
    public let opaqueValue: String

    public init(opaqueValue: String) {
        self.opaqueValue = opaqueValue
    }
}

public enum WorkbenchAccountConnectionKind: String, Equatable, Sendable {
    case localSigner
    case remoteSigner
    case readOnly

    public var title: String {
        switch self {
        case .localSigner:
            "On this Mac"
        case .remoteSigner:
            "Connected signer"
        case .readOnly:
            "Browsing only"
        }
    }
}

/// A bounded, non-secret account projection intended for native rendering.
public struct WorkbenchStoredAccount: Identifiable, Equatable, Sendable {
    public let handle: WorkbenchAccountHandle
    public let npub: String
    public let publicKeyHex: String
    public let connectionKind: WorkbenchAccountConnectionKind

    public var id: WorkbenchAccountHandle {
        handle
    }

    public init(
        handle: WorkbenchAccountHandle,
        npub: String,
        publicKeyHex: String,
        connectionKind: WorkbenchAccountConnectionKind
    ) {
        self.handle = handle
        self.npub = npub
        self.publicKeyHex = publicKeyHex
        self.connectionKind = connectionKind
    }
}

/// The active account projection. It contains public identity only.
public struct WorkbenchActiveAccount: Equatable, Sendable {
    public let handle: WorkbenchAccountHandle
    public let npub: String
    public let publicKeyHex: String
    public let connectionKind: WorkbenchAccountConnectionKind

    fileprivate init(account: WorkbenchStoredAccount) {
        handle = account.handle
        npub = account.npub
        publicKeyHex = account.publicKeyHex
        connectionKind = account.connectionKind
    }
}

public enum WorkbenchAccountAvailability: Equatable, Sendable {
    case available
    case unavailable(reason: String)
}

/// Screen-shaped account state. Rust owns the account-count ceiling and every
/// accepted or refused mutation; this projection does not re-derive either.
public struct WorkbenchAccountSnapshot: Equatable, Sendable {
    public let availability: WorkbenchAccountAvailability
    public let accounts: [WorkbenchStoredAccount]
    public let activeHandle: WorkbenchAccountHandle?
    public let errorMessage: String?

    public var activeAccount: WorkbenchActiveAccount? {
        guard
            let activeHandle,
            let account = accounts.first(where: { $0.handle == activeHandle })
        else {
            return nil
        }
        return WorkbenchActiveAccount(account: account)
    }

    public init?(
        availability: WorkbenchAccountAvailability = .available,
        accounts: [WorkbenchStoredAccount],
        activeHandle: WorkbenchAccountHandle?,
        errorMessage: String? = nil
    ) {
        if let activeHandle {
            guard accounts.contains(where: { $0.handle == activeHandle }) else {
                return nil
            }
        }
        self.availability = availability
        self.accounts = accounts
        self.activeHandle = activeHandle
        self.errorMessage = errorMessage
    }

    public static func unavailable(reason: String) -> Self {
        WorkbenchAccountSnapshot(
            availability: .unavailable(reason: reason),
            accounts: [],
            activeHandle: nil
        )!
    }
}

/// Native account capability used by the Workbench presentation.
///
/// Registration and selection remain separate runtime actions. A native view
/// may compose them into one user intent, but must confirm each projected
/// result before issuing the next action.
@MainActor
public protocol WorkbenchAccountManaging: AnyObject {
    func snapshot() -> WorkbenchAccountSnapshot
    func register(secret: String) async
    func registerReadOnly(publicIdentity: String) async
    func activate(handle: WorkbenchAccountHandle) async
    func logout() async
    func remove(handle: WorkbenchAccountHandle) async
}

/// Honest default until the application injects its Rust-backed adapter.
@MainActor
public final class UnavailableWorkbenchAccountManager:
    WorkbenchAccountManaging
{
    public static let defaultReason =
        "Account support is unavailable while the app is opening."

    private let reason: String

    public init(reason: String = defaultReason) {
        self.reason = reason
    }

    public func snapshot() -> WorkbenchAccountSnapshot {
        .unavailable(reason: reason)
    }

    public func register(secret _: String) async {}
    public func registerReadOnly(publicIdentity _: String) async {}
    public func activate(handle _: WorkbenchAccountHandle) async {}
    public func logout() async {}
    public func remove(handle _: WorkbenchAccountHandle) async {}
}
