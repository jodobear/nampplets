import AppKit
import SwiftUI
import RuntimeWorkbenchFeature

final class RuntimeWorkbenchAppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_: Notification) {
        activateWorkbenchWindow()
        DispatchQueue.main.async { [weak self] in
            self?.activateWorkbenchWindow()
        }
    }

    private func activateWorkbenchWindow() {
        NSApplication.shared.activate(ignoringOtherApps: true)
        NSApplication.shared.windows.first?.makeKeyAndOrderFront(nil)
    }
}

@main
struct RuntimeWorkbenchApp: App {
    @NSApplicationDelegateAdaptor(RuntimeWorkbenchAppDelegate.self)
    private var appDelegate
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
                        .frame(minWidth: 1_050, minHeight: 660)
                }
            }
            .onAppear {
                NSApplication.shared.activate(ignoringOtherApps: true)
            }
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
