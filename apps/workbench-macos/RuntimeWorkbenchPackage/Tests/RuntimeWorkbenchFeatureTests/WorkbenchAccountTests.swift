import Foundation
@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func unavailableAccountManagerRefusesEveryMutation() async {
    let reason = "Signer adapter not installed"
    let manager = UnavailableWorkbenchAccountManager(reason: reason)

    #expect(manager.snapshot() == .unavailable(reason: reason))

    await manager.register(secret: "not-a-real-secret")
    await manager.registerReadOnly(publicIdentity: "not-a-real-public-key")
    await manager.activate(
        handle: WorkbenchAccountHandle(opaqueValue: "account")
    )
    await manager.logout()
    await manager.remove(
        handle: WorkbenchAccountHandle(opaqueValue: "account")
    )

    #expect(manager.snapshot() == .unavailable(reason: reason))
}

@MainActor
@Test func publicIdentityUsesReadOnlyRegistrationBehindTheSameIntent()
    async throws
{
    let manager = RecordingAccountManager()
    let model = WorkbenchAccountSheetModel(manager: manager)

    model.identity = "npub1test"
    let succeeded = await model.continueWithIdentity()

    #expect(succeeded)
    #expect(model.identity.isEmpty)
    #expect(model.snapshot.accounts.count == 1)
    #expect(model.snapshot.accounts.first?.connectionKind == .readOnly)
    let handle = try #require(model.snapshot.accounts.first?.handle)
    #expect(model.snapshot.activeAccount?.handle == handle)
    #expect(manager.actions == [.registerReadOnly, .activate(handle)])
}

@Test func activeAccountIsDerivedFromOpaqueHandle() throws {
    let account = WorkbenchStoredAccount.fixture(handle: "account-a")
    let snapshot = try #require(WorkbenchAccountSnapshot(
        accounts: [account],
        activeHandle: account.handle
    ))

    #expect(snapshot.activeAccount?.handle == account.handle)
    #expect(snapshot.activeAccount?.npub == account.npub)
    #expect(snapshot.activeAccount?.publicKeyHex == account.publicKeyHex)
}

@Test func accountSnapshotRejectsInconsistentActiveHandle() {
    // Count is deliberately unbounded here: Rust (nmp-adapter's
    // MAX_PROFILE_ACCOUNTS) is the sole owner of that capacity limit and
    // enforces it before any snapshot reaches Swift -- this type must not
    // re-derive a competing ceiling. See issue #115.
    #expect(
        WorkbenchAccountSnapshot(
            accounts: [],
            activeHandle: WorkbenchAccountHandle(opaqueValue: "missing")
        ) == nil
    )
}

@MainActor
@Test func nsecUsesSigningRegistrationBehindTheSameIntent() async throws {
    let manager = RecordingAccountManager()
    let model = WorkbenchAccountSheetModel(manager: manager)

    model.identity = " \n nsec1test \t"
    let succeeded = await model.continueWithIdentity()

    #expect(succeeded)
    #expect(model.identity.isEmpty)
    #expect(model.snapshot.accounts.count == 1)
    let handle = try #require(model.snapshot.accounts.first?.handle)
    #expect(model.snapshot.activeAccount?.handle == handle)
    #expect(manager.actions == [.register, .activate(handle)])
}

@MainActor
@Test func registrationErrorCannotEchoSubmittedSecret() async {
    let manager = EchoingFailureAccountManager()
    let model = WorkbenchAccountSheetModel(manager: manager)
    let submittedSecret = "nsec1secret-that-must-not-render"

    model.identity = submittedSecret
    let succeeded = await model.continueWithIdentity()

    #expect(!succeeded)
    #expect(model.identity.isEmpty)
    #expect(model.errorMessage?.contains(submittedSecret) == false)
    #expect(model.errorMessage?.contains("wasn’t accepted") == true)
}

private extension WorkbenchStoredAccount {
    static func fixture(handle: String) -> WorkbenchStoredAccount {
        WorkbenchStoredAccount(
            handle: WorkbenchAccountHandle(opaqueValue: handle),
            npub: "npub1fixture\(handle)",
            publicKeyHex: String(repeating: "a", count: 64),
            connectionKind: .localSigner
        )
    }
}

@MainActor
private final class RecordingAccountManager: WorkbenchAccountManaging {
    enum Action: Equatable {
        case register
        case registerReadOnly
        case activate(WorkbenchAccountHandle)
    }

    private var accounts: [WorkbenchStoredAccount] = []
    private var activeHandle: WorkbenchAccountHandle?
    private(set) var actions: [Action] = []

    func snapshot() -> WorkbenchAccountSnapshot {
        WorkbenchAccountSnapshot(
            accounts: accounts,
            activeHandle: activeHandle
        )!
    }

    func register(secret: String) async {
        guard secret == "nsec1test" else { return }
        let account = WorkbenchStoredAccount.fixture(handle: "registered")
        accounts.append(account)
        actions.append(.register)
    }

    func registerReadOnly(publicIdentity: String) async {
        guard publicIdentity == "npub1test" else { return }
        let account = WorkbenchStoredAccount(
            handle: WorkbenchAccountHandle(
                opaqueValue: "registered-read-only"
            ),
            npub: "",
            publicKeyHex: String(repeating: "b", count: 64),
            connectionKind: .readOnly
        )
        accounts.append(account)
        actions.append(.registerReadOnly)
    }

    func activate(handle: WorkbenchAccountHandle) async {
        guard accounts.contains(where: { $0.handle == handle }) else { return }
        activeHandle = handle
        actions.append(.activate(handle))
    }

    func logout() async {
        activeHandle = nil
    }

    func remove(handle: WorkbenchAccountHandle) async {
        accounts.removeAll { $0.handle == handle }
        if activeHandle == handle {
            activeHandle = nil
        }
    }
}

@MainActor
private final class EchoingFailureAccountManager: WorkbenchAccountManaging {
    private var errorMessage: String?

    func snapshot() -> WorkbenchAccountSnapshot {
        WorkbenchAccountSnapshot(
            accounts: [],
            activeHandle: nil,
            errorMessage: errorMessage
        )!
    }

    func register(secret: String) async {
        errorMessage = "Rejected \(secret)"
    }

    func registerReadOnly(publicIdentity _: String) async {}
    func activate(handle _: WorkbenchAccountHandle) async {}
    func logout() async {}
    func remove(handle _: WorkbenchAccountHandle) async {}
}
