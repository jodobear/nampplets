import RuntimeWorkbenchFeature
import SwiftUI

@main
struct RuntimeWorkbenchiOSApp: App {
    @State private var runtimeProfile: WorkbenchRuntimeProfile?
    @State private var runtimeError: String?
    @State private var isOpeningRuntime = false

    var body: some Scene {
        WindowGroup {
            Group {
                if let runtimeProfile {
                    ContentView(profile: runtimeProfile)
                        .id(ObjectIdentifier(runtimeProfile))
                } else if let runtimeError {
                    ContentView(bootstrapError: runtimeError)
                } else {
                    ProgressView("Opening runtime…")
                }
            }
            .task {
                await openRuntimeIfNeeded()
            }
        }
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
        let uiTestScenario = ProcessInfo.processInfo.environment[
            "NMP_WORKBENCH_UI_TEST_SCENARIO"
        ]
        do {
            runtimeProfile = try await Task.detached {
                if let uiTestScenario {
                    return try WorkbenchRuntimeProfile.openForUITesting(
                        scenario: uiTestScenario
                    )
                }
                return try WorkbenchRuntimeProfile.openDefault()
            }.value
        } catch {
            runtimeError = error.localizedDescription
        }
    }
}
