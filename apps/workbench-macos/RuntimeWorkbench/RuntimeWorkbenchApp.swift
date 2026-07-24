import SwiftUI
import RuntimeWorkbenchFeature

@main
struct RuntimeWorkbenchApp: App {
    @State private var runtimeProfile: WorkbenchRuntimeProfile?
    @State private var runtimeError: String?
    @State private var isOpeningRuntime = false

    var body: some Scene {
        WindowGroup {
            ContentView(
                profile: runtimeProfile,
                bootstrapError: runtimeError
            )
            .task {
                await openRuntimeIfNeeded()
            }
        }
        .defaultSize(width: 1180, height: 780)
    }

    @MainActor
    private func openRuntimeIfNeeded() async {
        guard runtimeProfile == nil,
              runtimeError == nil,
              !isOpeningRuntime
        else {
            return
        }
        isOpeningRuntime = true
        defer { isOpeningRuntime = false }
        do {
            runtimeProfile = try await Task.detached {
                try WorkbenchRuntimeProfile.openDefault()
            }.value
        } catch {
            runtimeError = error.localizedDescription
        }
    }
}
