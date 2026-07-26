import Foundation
import Observation

@MainActor
@Observable
final class WorkbenchAccountSheetModel {
    private let manager: any WorkbenchAccountManaging

    private(set) var snapshot: WorkbenchAccountSnapshot
    private(set) var errorMessage: String?
    private(set) var isWorking = false
    var identity = ""

    init(manager: any WorkbenchAccountManaging) {
        self.manager = manager
        snapshot = manager.snapshot()
    }

    func refresh() {
        snapshot = manager.snapshot()
        errorMessage = nil
    }

    var identityLooksSecret: Bool {
        Self.identityKind(for: identity) == .signingKey
    }

    /// Maps one user intent onto the runtime's separate registration and
    /// selection actions. Only an nsec-shaped value is treated as signing
    /// material; every public identity shape takes the read-only path.
    ///
    /// The field is cleared before the first await so secret-bearing input
    /// cannot remain in observable presentation state during registration.
    func continueWithIdentity() async -> Bool {
        let submittedIdentity = identity.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        identity.removeAll(keepingCapacity: false)
        guard !submittedIdentity.isEmpty else {
            errorMessage = "Enter your account key or public profile."
            return false
        }

        switch Self.identityKind(for: submittedIdentity) {
        case .signingKey:
            return await registerAndSelect(
                registrationFailure:
                    "That account key wasn’t accepted. Check that you copied the complete key."
            ) {
                await manager.register(secret: submittedIdentity)
            }
        case .publicIdentity:
            return await registerAndSelect(
                registrationFailure:
                    "That profile couldn’t be added. Check the address and try again."
            ) {
                await manager.registerReadOnly(
                    publicIdentity: submittedIdentity
                )
            }
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
        errorMessage = nil
    }

    private enum IdentityKind {
        case signingKey
        case publicIdentity
    }

    private static func identityKind(for value: String) -> IdentityKind {
        value.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
            .hasPrefix("nsec1")
            ? .signingKey
            : .publicIdentity
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
