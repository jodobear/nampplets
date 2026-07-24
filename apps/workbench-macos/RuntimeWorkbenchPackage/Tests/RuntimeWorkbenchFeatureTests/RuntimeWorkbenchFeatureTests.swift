import Foundation
import Testing
@testable import RuntimeWorkbenchFeature

@MainActor
@Test func workbenchFeatureBuilds() {
    let view = ContentView()
    #expect(String(describing: type(of: view)) == "ContentView")
}

@MainActor
@Test func contentViewAcceptsInjectedInstalledLibraryManager() {
    let view = ContentView(
        libraryManager: UnavailableWorkbenchLibraryManager(
            reason: "Typed installed-library projection is unavailable."
        )
    )

    #expect(String(describing: type(of: view)) == "ContentView")
}

@Test func defaultLayoutHasFourVisibleBoundedSlots() {
    let layout = WorkbenchLayoutModel()

    #expect(
        WorkbenchSlotRole.allCases.allSatisfy {
            layout.isVisible($0)
        }
    )
    #expect(layout.component(in: .feed) == .goodMorning)
    #expect(layout.snapshot.focusedRole == .feed)

    for role in WorkbenchSlotRole.allCases {
        let size = layout.size(for: role)
        let constraints = role.constraints
        #expect(size.width >= constraints.minimumWidth)
        #expect(size.width <= constraints.maximumWidth)
        #expect(size.height >= constraints.minimumHeight)
        #expect(size.height <= constraints.maximumHeight)
    }
}

@Test func movingGoodMorningChangesItsRoleWithoutDuplicatingIt() {
    var layout = WorkbenchLayoutModel()

    layout.move(.goodMorning, to: .composer)

    #expect(layout.component(in: .feed) == nil)
    #expect(layout.component(in: .composer) == .goodMorning)
    #expect(layout.snapshot.focusedRole == .composer)
    #expect(layout.isVisible(.composer))
    #expect(
        WorkbenchSlotRole.allCases.filter {
            layout.component(in: $0) == .goodMorning
        }.count == 1
    )
}

@Test func hidingFocusedSlotSelectsAVisibleKeyboardFocusFallback() {
    var layout = WorkbenchLayoutModel()
    layout.focus(.tool)

    layout.setVisible(false, role: .tool)

    #expect(!layout.isVisible(.tool))
    #expect(layout.snapshot.focusedRole == .feed)
}

@Test func focusingHiddenSlotShowsIt() {
    var layout = WorkbenchLayoutModel()
    layout.setVisible(false, role: .detail)

    layout.focus(.detail)

    #expect(layout.isVisible(.detail))
    #expect(layout.snapshot.focusedRole == .detail)
}

@Test func renderedSizesAreClampedAndStableAtTheirBounds() {
    var layout = WorkbenchLayoutModel()

    let firstChange = layout.recordRenderedSize(
        role: .tool,
        width: 10_000,
        height: 1
    )
    let bounded = layout.size(for: .tool)
    let constraints = WorkbenchSlotRole.tool.constraints
    let secondChange = layout.recordRenderedSize(
        role: .tool,
        width: 10_000,
        height: 1
    )

    #expect(firstChange)
    #expect(!secondChange)
    #expect(bounded.width == Double(constraints.maximumWidth))
    #expect(bounded.height == Double(constraints.minimumHeight))
}

@Test func persistedSnapshotRoundTripsWithoutPlatformStorage() throws {
    var layout = WorkbenchLayoutModel()
    layout.move(.goodMorning, to: .tool)
    layout.setVisible(false, role: .detail)
    layout.recordRenderedSize(role: .composer, width: 800, height: 240)

    let data = try JSONEncoder().encode(layout.snapshot)
    let decoded = try JSONDecoder().decode(
        WorkbenchLayoutSnapshot.self,
        from: data
    )
    let restored = WorkbenchLayoutModel(snapshot: decoded)

    #expect(restored == layout)
}

@Test func unsupportedPersistedLayoutVersionFallsBackSafely() {
    var snapshot = WorkbenchLayoutSnapshot.workbenchDefault
    snapshot.version = WorkbenchLayoutSnapshot.currentVersion + 1
    snapshot.visibleRoles = []
    snapshot.assignments = [:]

    let restored = WorkbenchLayoutModel(snapshot: snapshot)

    #expect(restored.snapshot == .workbenchDefault)
}

@Test func malformedDuplicateComponentAssignmentIsNormalized() {
    var snapshot = WorkbenchLayoutSnapshot.workbenchDefault
    snapshot.assignments[.detail] = .goodMorning

    let restored = WorkbenchLayoutModel(snapshot: snapshot)

    #expect(restored.component(in: .feed) == .goodMorning)
    #expect(restored.component(in: .detail) == nil)
}

@MainActor
@Test func bundledSignedFixtureOpensThroughTheRustRuntime() throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "runtime-workbench-test-\(UUID().uuidString)",
            isDirectory: true
        )
    defer { try? FileManager.default.removeItem(at: root) }

    let fixture = try GoodMorningFixture.load()
    let profile = try WorkbenchRuntimeProfile.open(storageRoot: root)
    defer { profile.close() }
    let artifact = try installApproveAndLaunchGoodMorning(
        fixture: fixture,
        profile: profile
    )

    #expect(artifact.title == "Good Morning Protocol")
}
