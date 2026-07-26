import SwiftUI

enum WorkbenchSettingsSection: String, CaseIterable, Identifiable {
    case general
    case connections
    case storage

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: "General"
        case .connections: "Connections"
        case .storage: "Storage"
        }
    }

    var subtitle: String {
        switch self {
        case .general:
            "Choose how the app looks on this Mac."
        case .connections:
            "Choose where napplets connect and where the catalog looks."
        case .storage:
            "See what the app keeps on this Mac."
        }
    }

    var systemImage: String {
        switch self {
        case .general: "gearshape.fill"
        case .connections: "antenna.radiowaves.left.and.right"
        case .storage: "internaldrive.fill"
        }
    }

    var tint: Color {
        switch self {
        case .general: .gray
        case .connections: .blue
        case .storage: .indigo
        }
    }
}

struct WorkbenchSettingsSidebar: View {
    @Binding var selection: WorkbenchSettingsSection

    var body: some View {
        #if os(macOS)
        List(selection: $selection) {
            settingsRows
        }
        .listStyle(.sidebar)
        .navigationSplitViewColumnWidth(min: 190, ideal: 220, max: 250)
        #else
        List {
            settingsRows
        }
        .listStyle(.insetGrouped)
        #endif
    }

    @ViewBuilder
    private var settingsRows: some View {
        ForEach(WorkbenchSettingsSection.allCases) { section in
            #if os(macOS)
            WorkbenchSettingsSidebarRow(section: section)
                .tag(section)
            #else
            Button {
                selection = section
            } label: {
                WorkbenchSettingsSidebarRow(section: section)
            }
            .buttonStyle(.plain)
            #endif
        }
    }
}

private struct WorkbenchSettingsSidebarRow: View {
    let section: WorkbenchSettingsSection

    var body: some View {
        Label {
            Text(section.title)
        } icon: {
            WorkbenchSettingsIcon(
                systemImage: section.systemImage,
                tint: section.tint,
                size: 26
            )
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier("settings-\(section.rawValue)")
    }
}

struct WorkbenchSettingsIcon: View {
    let systemImage: String
    let tint: Color
    let size: CGFloat

    var body: some View {
        Image(systemName: systemImage)
            .font(.system(size: size * 0.54, weight: .semibold))
            .foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(tint.gradient, in: .rect(cornerRadius: size * 0.24))
            .overlay {
                RoundedRectangle(
                    cornerRadius: size * 0.24,
                    style: .continuous
                )
                .stroke(.white.opacity(0.18), lineWidth: 0.5)
            }
            .shadow(color: .black.opacity(0.15), radius: 1, y: 1)
            .accessibilityHidden(true)
    }
}
