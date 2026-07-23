import Foundation
import Testing
@testable import RuntimeWorkbenchFeature

@MainActor
@Test func workbenchFeatureBuilds() {
    let view = ContentView()
    #expect(String(describing: type(of: view)) == "ContentView")
}

@Test func bundledSignedFixtureOpensThroughTheRustRuntime() async throws {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "runtime-workbench-test-\(UUID().uuidString)",
            isDirectory: true
        )
    defer { try? FileManager.default.removeItem(at: root) }

    let fixture = try GoodMorningFixture.load()
    let artifact = try await Task.detached {
        try fixture.open(storageRoot: root)
    }.value

    #expect(artifact.title == "Good Morning Protocol")
}
