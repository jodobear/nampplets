import Foundation
import NMPNativeRuntimeApple
@testable import RuntimeWorkbenchFeature
import Testing

@MainActor
@Test func nativeLayoutAdapterRestoresWorkspaceAcrossProfileRestart() throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let workspaceID = "restart-proof"
    var expected = WorkbenchLayoutSnapshot.workbenchDefault
    expected.mode = .tiling
    expected.windows = [.goodMorning]
    expected.selectedWindowID = WorkbenchCanvasWindow.goodMorning.id
    expected.windows[0].frame = WorkbenchWindowFrame(
        x: 180,
        y: 90,
        width: 1_100,
        height: 620
    )

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

@MainActor
@Test
func persistedInstalledCanvasBuildIsPlannedReacquiredAndLaunched() throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let fixture = try GoodMorningFixture.load()
    let identity = WorkbenchExactBuildIdentity(
        manifestAuthor: GoodMorningFixture.author,
        dTag: GoodMorningFixture.dTag,
        aggregateHash: GoodMorningFixture.aggregateHash
    )
    let expected = WorkbenchLayoutSnapshot(
        mode: .freeform,
        windows: [.goodMorning],
        selectedWindowID: WorkbenchCanvasWindow.goodMorning.id
    )

    do {
        let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
        _ = try installApproveAndLaunchGoodMorning(
            fixture: fixture,
            profile: profile
        )
        let store = RuntimeWorkbenchLayoutStore(profile: profile)
        try store.saveLayout(expected, workspaceID: "default")
        profile.close()
    }

    do {
        let profile = try WorkbenchRuntimeProfile.open(
            storageRoot: root,
            persistedArtifactResolver: { native, _ in
                do {
                    let installed = try native.installSignedNamed(
                        title: "Good Morning Protocol",
                        eventJSON: fixture.eventJSON,
                        author: GoodMorningFixture.author,
                        dTag: GoodMorningFixture.dTag,
                        blobsBySHA256: [
                            GoodMorningFixture.indexDigest:
                                fixture.indexHTML,
                        ]
                    )
                    return native.reacquireInstalledArtifact(
                        installed.permissionCoordinate
                    )
                } catch {
                    return .refused(
                        NativeRuntimeCatalogFailure(
                            code: "test-source-refused",
                            detail: error.localizedDescription,
                            provenance: []
                        )
                    )
                }
            }
        )
        defer { profile.close() }
        let store = RuntimeWorkbenchLayoutStore(profile: profile)
        let restored = try #require(
            try store.loadLayout(workspaceID: "default")
        )
        let plan = WorkbenchRestoredCanvasLaunchPlan(
            layout: WorkbenchLayoutModel(snapshot: restored)
        )
        #expect(plan.identities == [identity])

        // A restarted profile no longer needs a network-backed resolver to
        // reopen an installed exact build: Rust re-verifies the signed
        // manifest event it retained at install time against the sealed
        // artifact cache, entirely offline.
        guard case let .installed(directly) =
            profile.reacquireInstalledArtifact(for: identity)
        else {
            Issue.record(
                "A restarted profile must reopen the sealed exact build offline"
            )
            return
        }
        #expect(
            directly.installedArtifact.permissionCoordinate.aggregateHash
                == GoodMorningFixture.aggregateHash
        )

        let installation = try #require(
            reacquiredInstallation(
                profile.reacquirePersistedCanvasArtifact(
                    for: plan.identities[0]
                )
            )
        )
        let review = profile.native.permissionReview(
            for: installation.installedArtifact.permissionCoordinate
        )
        #expect(review.refusal == nil)
        #expect(review.review?.launchPermitted == true)

        let launched = try profile.native.launchInstalled(
            installation.installedArtifact
        )
        #expect(launched.title == "Good Morning Protocol")
    }
}

@MainActor
@Test func nativeLayoutAdapterPersistsRetainedReceiptIDsForRestart() throws {
    let root = temporaryRuntimeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let workspaceID = "receipt-restart-proof"
    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let store: any WorkbenchLayoutPersisting =
        RuntimeWorkbenchLayoutStore(profile: profile)

    try store.saveLayout(
        WorkbenchLayoutSnapshot(
            mode: .freeform,
            windows: [.goodMorning],
            selectedWindowID: WorkbenchCanvasWindow.goodMorning.id
        ),
        workspaceID: workspaceID,
        retainedReceiptIDs: ["receipt-1", "receipt-2"]
    )
    let restored = try #require(
        profile.native.restoreWorkspaces().workspaces.first {
            $0.workspaceId == workspaceID
        }
    )
    #expect(restored.retainedReceiptIds == ["receipt-1", "receipt-2"])
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

private func reacquiredInstallation(
    _ result: NativeRuntimeCatalogInstallResult
) -> NativeRuntimeCatalogInstallation? {
    switch result {
    case let .installed(installation):
        installation
    case .refused:
        nil
    }
}
