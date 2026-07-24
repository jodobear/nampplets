import Foundation
@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func nativeAccountAdapterKeepsRegistrationAndActivationExplicit() async throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }

    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let manager = RuntimeWorkbenchAccountManager(profile: profile)

    #expect(manager.snapshot().accounts.isEmpty)
    #expect(manager.snapshot().activeAccount == nil)

    await manager.register(secret: String(format: "%064x", 7))
    let registered = try #require(manager.snapshot().accounts.first)
    #expect(manager.snapshot().accounts.count == 1)
    #expect(manager.snapshot().activeAccount == nil)
    #expect(registered.publicKeyHex.count == 64)
    #expect(registered.npub.isEmpty)

    await manager.activate(handle: registered.handle)
    #expect(manager.snapshot().activeAccount?.handle == registered.handle)

    await manager.logout()
    #expect(manager.snapshot().activeAccount == nil)
    #expect(manager.snapshot().accounts == [registered])

    await manager.remove(handle: registered.handle)
    #expect(manager.snapshot().accounts.isEmpty)
    #expect(manager.snapshot().activeAccount == nil)
}

@MainActor
@Test func nativeLayoutAdapterRestoresWorkspaceAcrossProfileRestart() throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let workspaceID = "restart-proof"
    var expected = WorkbenchLayoutSnapshot.workbenchDefault
    expected.visibleRoles.remove(.tool)
    expected.focusedRole = .composer
    expected.assignments = [.composer: .goodMorning]
    expected.sizes[.composer] = WorkbenchSlotSize(width: 1_100, height: 260)

    do {
        let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
        let store = RuntimeWorkbenchLayoutStore(profile: profile)
        try store.saveLayout(expected, workspaceID: workspaceID)
        profile.close()
    }

    do {
        let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
        defer { profile.close() }
        let store = RuntimeWorkbenchLayoutStore(profile: profile)
        let loaded = try store.loadLayout(workspaceID: workspaceID)
        let restored = try #require(loaded)
        #expect(restored == expected)
    }
}

@Test func workbenchDefaultUsesAStableProfileScopedKeychainNamespace() throws {
    let firstRoot = URL(fileURLWithPath: "/private/workbench/profile-a")
    let secondRoot = URL(fileURLWithPath: "/private/workbench/profile-b")
    let first = WorkbenchRuntimeProfile.keychainPersistence(
        storageRoot: firstRoot
    )
    let repeated = WorkbenchRuntimeProfile.keychainPersistence(
        storageRoot: firstRoot
    )
    let second = WorkbenchRuntimeProfile.keychainPersistence(
        storageRoot: secondRoot
    )

    #expect(first == repeated)
    #expect(first != second)
    guard case let .keychain(namespace) = first else {
        Issue.record("Workbench default persistence was not Keychain-backed")
        return
    }
    #expect(namespace == firstRoot.standardizedFileURL.path)
}

private func temporaryRuntimeRoot() -> URL {
    FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "nmp-native-runtime-workbench-\(UUID().uuidString)",
            isDirectory: true
        )
}
