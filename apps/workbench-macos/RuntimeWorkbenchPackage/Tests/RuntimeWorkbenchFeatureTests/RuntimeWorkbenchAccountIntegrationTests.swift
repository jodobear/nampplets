import Foundation
import NMPNativeRuntimeApple
@testable import RuntimeWorkbenchFeature
import Testing

private let validTestNsec =
    "nsec1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqsmhltgl"

@MainActor
@Test func accountIntentAcceptsPastedNsecAndSelectsTheNewAccount() async throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }

    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let manager = RuntimeWorkbenchAccountManager(profile: profile)
    let model = WorkbenchAccountSheetModel(manager: manager)

    model.identity = "\n  \(validTestNsec) \t"
    let succeeded = await model.continueWithIdentity()

    #expect(succeeded)
    #expect(model.identity.isEmpty)
    let registered = try #require(manager.snapshot().accounts.first)
    #expect(manager.snapshot().accounts.count == 1)
    #expect(manager.snapshot().activeHandle == registered.handle)
    #expect(registered.publicKeyHex.count == 64)
}

@MainActor
@Test func nativeAccountAdapterKeepsRuntimeActionsSeparate() async throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }

    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let manager = RuntimeWorkbenchAccountManager(profile: profile)

    await manager.register(secret: validTestNsec)
    let registered = try #require(manager.snapshot().accounts.first)
    #expect(manager.snapshot().activeAccount == nil)

    await manager.activate(handle: registered.handle)
    #expect(manager.snapshot().activeAccount?.handle == registered.handle)

    await manager.logout()
    #expect(manager.snapshot().activeAccount == nil)
    #expect(manager.snapshot().accounts == [registered])

    await manager.remove(handle: registered.handle)
    #expect(manager.snapshot().accounts.isEmpty)

    await manager.registerReadOnly(
        publicIdentity:
            "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6"
    )
    let readOnly = try #require(manager.snapshot().accounts.first)
    #expect(
        readOnly.connectionKind == WorkbenchAccountConnectionKind.readOnly
    )
    #expect(manager.snapshot().activeAccount == nil)

    await manager.activate(handle: readOnly.handle)
    #expect(
        manager.snapshot().activeAccount?.connectionKind == .readOnly
    )

    await manager.registerReadOnly(publicIdentity: "pablo@example.com")
    #expect(
        manager.snapshot().errorMessage?.contains(
            "pinned NMP facade cannot resolve NIP-05"
        ) == true
    )
    #expect(manager.snapshot().activeAccount?.handle == readOnly.handle)
}

@MainActor
@Test
func accountManagerRendersRustsFullSnapshotAtCapacityWithoutDiscardingIt()
    async throws
{
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }

    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let manager = RuntimeWorkbenchAccountManager(profile: profile)

    var registered = 0
    while manager.snapshot().errorMessage == nil {
        registered += 1
        guard registered < 10_000 else {
            Issue.record("Rust never refused registration")
            return
        }
        await manager.register(secret: String(format: "%064x", registered))
    }
    let acceptedCount = registered - 1
    let limit = manager.snapshot().accounts.count
    #expect(acceptedCount == limit)
    #expect(
        manager.snapshot().errorMessage?.contains(
            "The account registry is full at \(limit) entries."
        ) == true
    )
    guard case .available = manager.snapshot().availability else {
        Issue.record(
            "A full Rust snapshot must remain available to the presentation."
        )
        return
    }

    await manager.registerReadOnly(
        publicIdentity:
            "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5"
    )

    #expect(
        manager.snapshot().errorMessage?.contains(
            "The account registry is full at \(limit) entries."
        ) == true
    )
    #expect(manager.snapshot().accounts.count == limit)
    guard case .available = manager.snapshot().availability else {
        Issue.record(
            "A later capacity refusal must not erase accepted accounts."
        )
        return
    }
}
