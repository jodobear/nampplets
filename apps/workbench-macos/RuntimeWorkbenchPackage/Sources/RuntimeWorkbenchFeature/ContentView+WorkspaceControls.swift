import SwiftUI

extension ContentView {
    var workspaceActionsMenu: some View {
        Menu {
            Button("Installed Napplets", systemImage: "square.stack.3d.up") {
                isLibrarySheetPresented = true
            }
            .keyboardShortcut("l", modifiers: [.command, .shift])

            Button("Activity", systemImage: "waveform.path.ecg") {
                openActivityDrawer()
            }
            .keyboardShortcut("a", modifiers: [.command, .shift])

            Button("Permissions", systemImage: "lock.shield") {
                openPermissionReview()
            }
            .keyboardShortcut("p", modifiers: [.command, .shift])

            Divider()

            Button("Settings", systemImage: "gearshape") {
                openSettings()
            }
            .keyboardShortcut(",", modifiers: [.command])
        } label: {
            Label("Workspace Actions", systemImage: "ellipsis.circle")
        }
        .labelStyle(.iconOnly)
        .menuStyle(.borderlessButton)
        .accessibilityLabel("Workspace Actions")
        .accessibilityIdentifier("workspace-actions")
        .accessibilityHint(
            "Opens installed napplets, activity, permissions, or settings"
        )
    }

    var layoutMenu: some View {
        Menu {
            Section("Window layout") {
                ForEach(availableLayoutModes, id: \.self) { mode in
                    Button {
                        setLayoutMode(mode)
                    } label: {
                        if layout.mode == mode {
                            Label(mode.title, systemImage: "checkmark")
                        } else {
                            Label(mode.title, systemImage: mode.systemImage)
                        }
                    }
                }
            }
        } label: {
            Label(layout.mode.title, systemImage: layout.mode.systemImage)
        }
        .accessibilityHint(
            "Switches between freely arranged, automatically tiled, and full "
                + "window napplet display"
        )
        .accessibilityIdentifier("layout-mode-menu")
    }

    private var availableLayoutModes: [WorkbenchLayoutMode] {
        #if os(iOS)
        WorkbenchLayoutMode.allCases
        #else
        WorkbenchLayoutMode.allCases.filter { $0 != .fullWindow }
        #endif
    }
}
