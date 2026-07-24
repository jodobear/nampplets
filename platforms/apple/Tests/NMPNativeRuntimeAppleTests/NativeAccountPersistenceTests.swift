import Foundation
@testable import NMPNativeRuntimeApple
import Testing

private final class FakeNativeAccountVault:
    NativeAccountVault,
    @unchecked Sendable
{
    enum Failure: Equatable {
        case load
        case upsert
        case setActive
        case remove
    }

    private let lock = NSLock()
    private var stored = NativeAccountVaultSnapshot(
        accounts: [],
        activePublicKey: nil
    )
    private var failure: Failure?
    private var requestedLoadLimits: [Int] = []

    func fail(_ nextFailure: Failure?) {
        lock.lock()
        failure = nextFailure
        lock.unlock()
    }

    func snapshot() -> NativeAccountVaultSnapshot {
        lock.lock()
        let snapshot = stored
        lock.unlock()
        return snapshot
    }

    func replace(_ snapshot: NativeAccountVaultSnapshot) {
        lock.lock()
        stored = snapshot
        lock.unlock()
    }

    func loadLimits() -> [Int] {
        lock.lock()
        let limits = requestedLoadLimits
        lock.unlock()
        return limits
    }

    func load(
        maximumAccounts: Int
    ) throws -> NativeAccountVaultSnapshot {
        lock.lock()
        defer { lock.unlock() }
        requestedLoadLimits.append(maximumAccounts)
        try refuseIfNeeded(.load)
        guard stored.accounts.count <= maximumAccounts else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        return stored
    }

    func upsertLocalSigner(
        publicKey: String,
        secret: String,
        maximumAccounts: Int
    ) throws {
        lock.lock()
        defer { lock.unlock() }
        try refuseIfNeeded(.upsert)
        var accounts = stored.accounts.filter {
            $0.publicKey != publicKey
        }
        guard accounts.count < maximumAccounts else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        accounts.append(
            NativeStoredAccount(
                publicKey: publicKey,
                material: .localSigner(secret: secret)
            )
        )
        accounts.sort { $0.publicKey < $1.publicKey }
        stored = NativeAccountVaultSnapshot(
            accounts: accounts,
            activePublicKey: stored.activePublicKey
        )
    }

    func upsertReadOnly(
        publicKey: String,
        maximumAccounts: Int
    ) throws {
        lock.lock()
        defer { lock.unlock() }
        try refuseIfNeeded(.upsert)
        var accounts = stored.accounts.filter {
            $0.publicKey != publicKey
        }
        guard accounts.count < maximumAccounts else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        accounts.append(
            NativeStoredAccount(
                publicKey: publicKey,
                material: .readOnly
            )
        )
        accounts.sort { $0.publicKey < $1.publicKey }
        stored = NativeAccountVaultSnapshot(
            accounts: accounts,
            activePublicKey: stored.activePublicKey
        )
    }

    func setActive(
        publicKey: String?,
        maximumAccounts: Int
    ) throws {
        lock.lock()
        defer { lock.unlock() }
        try refuseIfNeeded(.setActive)
        guard stored.accounts.count <= maximumAccounts else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        if let publicKey,
           !stored.accounts.contains(where: {
               $0.publicKey == publicKey
           }) {
            throw NativeAccountVaultError.unknownAccount
        }
        stored = NativeAccountVaultSnapshot(
            accounts: stored.accounts,
            activePublicKey: publicKey
        )
    }

    func remove(
        publicKey: String,
        maximumAccounts: Int
    ) throws {
        lock.lock()
        defer { lock.unlock() }
        try refuseIfNeeded(.remove)
        guard stored.accounts.count <= maximumAccounts else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        stored = NativeAccountVaultSnapshot(
            accounts: stored.accounts.filter {
                $0.publicKey != publicKey
            },
            activePublicKey: stored.activePublicKey == publicKey
                ? nil
                : stored.activePublicKey
        )
    }

    private func refuseIfNeeded(_ operation: Failure) throws {
        guard failure == operation else {
            return
        }
        throw NativeAccountVaultError.keychain(status: -50)
    }
}

@Test func persistedAccountsRestoreAndOnlyStoredSelectionReactivates()
    throws
{
    let root = temporaryAccountPersistenceRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let vault = FakeNativeAccountVault()
    let firstSecret = String(format: "%064x", 11)
    let secondSecret = String(format: "%064x", 12)
    var firstPublicKey = ""
    var secondPublicKey = ""

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        let first = profile.registerLocalAccount(secretKey: firstSecret)
        let second = profile.registerLocalAccount(secretKey: secondSecret)
        let firstHandle = try #require(first.handle)
        let secondHandle = try #require(second.handle)
        firstPublicKey = firstHandle.publicKey
        secondPublicKey = secondHandle.publicKey

        #expect(first.accepted)
        #expect(second.accepted)
        #expect(profile.accountSnapshot().snapshot?.activePublicKey == nil)
        #expect(vault.snapshot().activePublicKey == nil)

        let activation = profile.activateLocalAccount(handle: secondHandle)
        #expect(activation.accepted)
        #expect(
            activation.snapshot?.activePublicKey == secondPublicKey
        )
        #expect(vault.snapshot().activePublicKey == secondPublicKey)
        profile.close()
    }

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        let restored = try #require(profile.accountSnapshot().snapshot)

        #expect(restored.localAccounts.count == 2)
        #expect(
            Set(restored.localAccounts.map(\.publicKey))
                == Set([firstPublicKey, secondPublicKey])
        )
        #expect(restored.activePublicKey == secondPublicKey)
        #expect(restored.activePublicKey != firstPublicKey)
        #expect(profile.accountPersistenceIssue() == nil)

        let logout = profile.logoutLocalAccount()
        #expect(logout.accepted)
        #expect(logout.snapshot?.activePublicKey == nil)
        #expect(vault.snapshot().activePublicKey == nil)
        profile.close()
    }

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        let restored = try #require(profile.accountSnapshot().snapshot)
        #expect(restored.localAccounts.count == 2)
        #expect(restored.activePublicKey == nil)

        let firstHandle = try #require(
            restored.localAccounts.first {
                $0.publicKey == firstPublicKey
            }
        )
        #expect(profile.activateLocalAccount(handle: firstHandle).accepted)
        #expect(profile.removeLocalAccount(handle: firstHandle).accepted)
        #expect(
            !vault.snapshot().accounts.contains {
                $0.publicKey == firstPublicKey
            }
        )
        #expect(vault.snapshot().activePublicKey == nil)
        profile.close()
    }

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        defer { profile.close() }
        let restored = try #require(profile.accountSnapshot().snapshot)
        #expect(restored.localAccounts.map(\.publicKey) == [secondPublicKey])
        #expect(restored.activePublicKey == nil)
    }
    #expect(vault.loadLimits().allSatisfy { $0 == 32 })
}

@Test func keylessReadOnlyAccountPersistsRestoresAndReactivates() throws {
    let root = temporaryAccountPersistenceRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let vault = FakeNativeAccountVault()
    let npub =
        "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
    var publicKey = ""

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        let registration = profile.registerReadOnlyAccount(
            publicIdentity: npub
        )
        let handle = try #require(registration.handle)
        publicKey = handle.publicKey
        #expect(registration.accepted)
        #expect(handle.kind == .readOnly)
        #expect(registration.snapshot?.activePublicKey == nil)
        #expect(profile.activateLocalAccount(handle: handle).accepted)
        #expect(vault.snapshot().activePublicKey == publicKey)
        #expect(
            vault.snapshot().accounts
                == [
                    NativeStoredAccount(
                        publicKey: publicKey,
                        material: .readOnly
                    ),
                ]
        )
        profile.close()
    }

    do {
        let profile = try openTestProfile(root: root, vault: vault)
        defer { profile.close() }
        let restored = try #require(profile.accountSnapshot().snapshot)
        let account = try #require(restored.localAccounts.first)
        #expect(restored.localAccounts.count == 1)
        #expect(account.publicKey == publicKey)
        #expect(account.kind == .readOnly)
        #expect(restored.activePublicKey == publicKey)
    }
}

@Test func vaultFailuresDoNotBlockReadOnlyStartupOrExposeSecrets()
    throws
{
    let root = temporaryAccountPersistenceRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let vault = FakeNativeAccountVault()
    let secret = String(format: "%064x", 19)

    vault.fail(.load)
    do {
        let profile = try openTestProfile(root: root, vault: vault)
        defer { profile.close() }
        #expect(profile.accountSnapshot().accepted)
        #expect(profile.accountSnapshot().snapshot?.localAccounts.isEmpty == true)
        #expect(profile.accountPersistenceIssue() == .restoreFailed)
        vault.fail(.upsert)
        #expect(profile.registerLocalAccount(secretKey: secret).accepted)
        #expect(profile.accountPersistenceIssue() == .restoreFailed)
    }

    vault.fail(.upsert)
    do {
        let profile = try openTestProfile(root: root, vault: vault)
        defer { profile.close() }
        let registration = profile.registerLocalAccount(secretKey: secret)

        #expect(registration.accepted)
        #expect(registration.snapshot?.localAccounts.count == 1)
        #expect(profile.accountPersistenceIssue() == .registerFailed)
        let description = profile.accountPersistenceIssue()?
            .errorDescription ?? ""
        #expect(!description.contains(secret))
        #expect(!String(describing: profile.accountPersistenceIssue()).contains(secret))
        #expect(vault.snapshot().accounts.isEmpty)
    }
}

@Test func restoreRequestsExactlyTheBoundedMaximumOfThirtyTwoAccounts()
    throws
{
    let root = temporaryAccountPersistenceRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let vault = FakeNativeAccountVault()
    vault.replace(
        NativeAccountVaultSnapshot(
            accounts: (1...33).map { index in
                NativeStoredAccount(
                    publicKey: String(format: "%064x", index),
                    material: .localSigner(
                        secret: String(format: "%064x", index)
                    )
                )
            },
            activePublicKey: nil
        )
    )

    let profile = try openTestProfile(root: root, vault: vault)
    defer { profile.close() }

    #expect(vault.loadLimits() == [32])
    #expect(profile.accountSnapshot().snapshot?.localAccounts.isEmpty == true)
    #expect(profile.accountPersistenceIssue() == .restoreFailed)
}

@Test func invalidKeychainNamespaceIsRefusedBeforeOpeningAProfile() {
    let root = temporaryAccountPersistenceRoot()
    defer { try? FileManager.default.removeItem(at: root) }

    #expect(throws: RuntimeNappletOpenError.invalidAccountPersistence) {
        try NativeRuntimeProfile.open(
            configuration: NativeRuntimeProfileConfiguration(
                storageRoot: root,
                accountPersistence: .keychain(namespace: "")
            )
        )
    }
}

private func openTestProfile(
    root: URL,
    vault: FakeNativeAccountVault
) throws -> NativeRuntimeProfile {
    try NativeRuntimeProfile.open(
        configuration: NativeRuntimeProfileConfiguration(
            storageRoot: root,
            accountPersistence: .transient
        ),
        accountVault: vault
    )
}

private func temporaryAccountPersistenceRoot() -> URL {
    FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "nmp-native-runtime-account-persistence-\(UUID().uuidString)",
            isDirectory: true
        )
}
