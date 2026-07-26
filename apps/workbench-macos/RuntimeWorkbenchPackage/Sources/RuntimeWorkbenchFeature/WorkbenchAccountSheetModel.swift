import Foundation
import Observation

@MainActor
@Observable
final class WorkbenchAccountSheetModel {
    private let manager: any WorkbenchAccountManaging

    private(set) var snapshot: WorkbenchAccountSnapshot
    private(set) var errorMessage: String?
    private(set) var isWorking = false
    var secret = ""
    var publicIdentity = ""

    init(manager: any WorkbenchAccountManaging) {
        self.manager = manager
        snapshot = manager.snapshot()
    }

    func refresh() {
        snapshot = manager.snapshot()
        errorMessage = nil
    }

    /// Maps one user intent onto the runtime's separate register and select
    /// actions without letting secret-bearing input survive the first await.
    func continueWithSecret() async -> Bool {
        let submittedSecret = secret.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        secret.removeAll(keepingCapacity: false)
        guard !submittedSecret.isEmpty else {
            errorMessage = "Paste the complete secret account key."
            return false
        }

        return await registerAndSelect(
            registrationFailure:
                "That account key wasn’t accepted. Check that you copied the complete key."
        ) {
            await manager.register(secret: submittedSecret)
        }
    }

    func continueReadOnly() async -> Bool {
        let submittedIdentity = publicIdentity.trimmingCharacters(
            in: .whitespacesAndNewlines
        )
        guard !submittedIdentity.isEmpty else {
            errorMessage = "Enter the profile you want to browse as."
            return false
        }

        let succeeded = await registerAndSelect(
            registrationFailure:
                "That profile couldn’t be added. Check the address and try again."
        ) {
            await manager.registerReadOnly(publicIdentity: submittedIdentity)
        }
        if succeeded {
            publicIdentity.removeAll(keepingCapacity: false)
        }
        return succeeded
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
        secret.removeAll(keepingCapacity: false)
        publicIdentity.removeAll(keepingCapacity: false)
        errorMessage = nil
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
