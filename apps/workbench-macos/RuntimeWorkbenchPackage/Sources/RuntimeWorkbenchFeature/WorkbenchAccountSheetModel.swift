import Foundation
import Observation

enum WorkbenchAccountIdentityUse: Hashable {
    case signing
    case readOnly
}

@MainActor
@Observable
final class WorkbenchAccountSheetModel {
    private let manager: any WorkbenchAccountManaging

    private(set) var snapshot: WorkbenchAccountSnapshot
    private(set) var errorMessage: String?
    private(set) var isWorking = false
    var identity = "" {
        didSet {
            guard identity != oldValue else {
                return
            }
            ambiguousIdentityUse = nil
            errorMessage = nil
        }
    }
    var ambiguousIdentityUse: WorkbenchAccountIdentityUse?

    init(manager: any WorkbenchAccountManaging) {
        self.manager = manager
        snapshot = manager.snapshot()
    }

    func refresh() {
        snapshot = manager.snapshot()
        errorMessage = nil
    }

    var requiresIdentityUseChoice: Bool {
        Self.identityKind(for: identity) == .ambiguousHex
    }

    /// Maps one user intent onto the runtime's separate registration and
    /// selection actions. Prefix-bearing identities choose an unambiguous
    /// path. A bare 64-character hex value can be either a secret or a public
    /// key, so it never reaches either registration path without an explicit
    /// user choice.
    ///
    /// The field is cleared before the first await so secret-bearing input
    /// cannot remain in observable presentation state during registration.
    func continueWithIdentity() async -> Bool {
        let submittedIdentity = identity.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        guard !submittedIdentity.isEmpty else {
            errorMessage = "Enter your account key or public profile."
            return false
        }

        let identityKind = Self.identityKind(for: submittedIdentity)
        let selectedUse = ambiguousIdentityUse
        if identityKind == .ambiguousHex, selectedUse == nil {
            errorMessage = "Choose how you use this key."
            return false
        }

        identity.removeAll(keepingCapacity: false)
        ambiguousIdentityUse = nil

        switch (identityKind, selectedUse) {
        case (.signingKey, _):
            return await registerSigning(submittedIdentity)
        case (.ambiguousHex, .some(.signing)):
            return await registerSigning(submittedIdentity)
        case (.publicIdentity, _), (.ambiguousHex, .some(.readOnly)):
            return await registerReadOnly(submittedIdentity)
        case (.ambiguousHex, .none):
            preconditionFailure("Ambiguous account input requires a choice")
        }
    }

    private func registerSigning(_ identity: String) async -> Bool {
        await registerAndSelect(
            registrationFailure:
                "That account key wasn’t accepted. Check that you copied the complete key."
        ) {
            await manager.register(secret: identity)
        }
    }

    private func registerReadOnly(_ identity: String) async -> Bool {
        await registerAndSelect(
            registrationFailure:
                "That profile couldn’t be added. Check the address and try again."
        ) {
            await manager.registerReadOnly(publicIdentity: identity)
        }
    }

    func activate(_ handle: WorkbenchAccountHandle) async {
        await perform(
            failureMessage: "That account couldn’t be selected.",
            succeeded: { $0.activeHandle == handle }
        ) {
            await manager.activate(handle: handle)
        }
    }

    func logout() async {
        await perform(
            failureMessage: "The account couldn’t be signed out.",
            succeeded: { $0.activeHandle == nil }
        ) {
            await manager.logout()
        }
    }

    func remove(_ handle: WorkbenchAccountHandle) async {
        await perform(
            failureMessage: "The account couldn’t be removed.",
            succeeded: {
                !$0.accounts.contains(where: { $0.handle == handle })
            }
        ) {
            await manager.remove(handle: handle)
        }
    }

    func clearTransientState() {
        identity.removeAll(keepingCapacity: false)
        ambiguousIdentityUse = nil
        errorMessage = nil
    }

    private enum IdentityKind {
        case signingKey
        case publicIdentity
        case ambiguousHex
    }

    private static func identityKind(for value: String) -> IdentityKind {
        let normalized = value.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        if isBareHex(normalized, exactLength: 64) {
            return .ambiguousHex
        }
        return normalized.lowercased().hasPrefix("nsec1")
            ? .signingKey
            : .publicIdentity
    }

    private static func isBareHex(
        _ value: String,
        exactLength: Int? = nil
    ) -> Bool {
        if let exactLength, value.utf8.count != exactLength {
            return false
        }
        return !value.isEmpty && value.utf8.allSatisfy {
            (48 ... 57).contains($0)
                || (65 ... 70).contains($0)
                || (97 ... 102).contains($0)
        }
    }

    private func registerAndSelect(
        registrationFailure: String,
        registration: () async -> Void
    ) async -> Bool {
        guard !isWorking else {
            return false
        }
        isWorking = true
        defer { isWorking = false }
        errorMessage = nil

        let existingHandles = Set(snapshot.accounts.map(\.handle))
        await registration()

        var nextSnapshot = manager.snapshot()
        snapshot = nextSnapshot
        let addedAccounts = nextSnapshot.accounts.filter {
            !existingHandles.contains($0.handle)
        }

        guard addedAccounts.count == 1, let addedAccount = addedAccounts.first
        else {
            errorMessage = nextSnapshot.errorMessage == nil
                ? "That account is already saved. Choose it from the account menu."
                : registrationFailure
            return false
        }

        await manager.activate(handle: addedAccount.handle)
        nextSnapshot = manager.snapshot()
        snapshot = nextSnapshot
        guard nextSnapshot.activeHandle == addedAccount.handle else {
            errorMessage = "The account was added, but couldn’t be selected."
            return false
        }
        return true
    }

    private func perform(
        failureMessage: String,
        succeeded: (WorkbenchAccountSnapshot) -> Bool,
        operation: () async -> Void
    ) async {
        guard !isWorking else {
            return
        }
        isWorking = true
        defer { isWorking = false }
        errorMessage = nil

        await operation()
        snapshot = manager.snapshot()
        if !succeeded(snapshot) {
            errorMessage = failureMessage
        }
    }
}
