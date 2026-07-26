import SwiftUI

struct WorkbenchSettingsDetail: View {
    let section: WorkbenchSettingsSection
    @Binding var appearance: WorkbenchAppearance
    @Binding var draft: WorkbenchProfilePreferences
    let storage: WorkbenchStorageSummary?
    let unavailableReason: String?
    let actionError: String?
    let isBusy: Bool
    let hasChanges: Bool
    let applyChanges: () -> Void
    let discardChanges: () -> Void
    let requestCacheClear: () -> Void

    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                pageHeader

                if let message = actionError ?? unavailableReason {
                    settingsNotice(message)
                }

                switch section {
                case .general:
                    generalPage
                case .connections:
                    connectionsPage
                case .storage:
                    storagePage
                }
            }
            .frame(maxWidth: 680)
            .padding(.horizontal, 44)
            .padding(.top, 30)
            .padding(.bottom, 48)
            .frame(maxWidth: .infinity)
        }
        .background(Color.primary.opacity(0.018))
        .accessibilityIdentifier("settings-\(section.rawValue)-detail")
    }

    private var pageHeader: some View {
        VStack(spacing: 10) {
            WorkbenchSettingsIcon(
                systemImage: section.systemImage,
                tint: section.tint,
                size: 64
            )
            Text(section.title)
                .font(.system(size: 30, weight: .bold, design: .rounded))
            Text(section.subtitle)
                .font(.body)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.bottom, 4)
    }

    private var generalPage: some View {
        WorkbenchSettingsCard {
            HStack(spacing: 16) {
                settingsRowIcon("circle.lefthalf.filled")
                VStack(alignment: .leading, spacing: 3) {
                    Text("Appearance")
                        .font(.body.weight(.medium))
                    Text("Follow your Mac or choose a fixed appearance.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 24)
                Picker("Appearance", selection: $appearance) {
                    ForEach(WorkbenchAppearance.allCases) { option in
                        Text(option.title).tag(option)
                    }
                }
                .labelsHidden()
                .pickerStyle(.segmented)
                .frame(width: 270)
                .accessibilityIdentifier("settings-appearance")
            }
        }
    }

    private var connectionsPage: some View {
        VStack(spacing: 16) {
            WorkbenchSettingsCard {
                WorkbenchRelayLaneEditor(
                    title: "App relays",
                    detail: "Keep your napplets connected and in sync.",
                    systemImage: "point.3.connected.trianglepath.dotted",
                    identifierPrefix: "app",
                    relays: $draft.appRelays
                )
            }
            WorkbenchSettingsCard {
                WorkbenchRelayLaneEditor(
                    title: "Indexer relays",
                    detail: "Find napplets and public profiles.",
                    systemImage: "magnifyingglass",
                    identifierPrefix: "indexer",
                    relays: $draft.indexerRelays
                )
            }
            Text("Relay addresses must begin with wss://.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 4)
            changesFooter
        }
        .disabled(unavailableReason != nil)
    }

    private var storagePage: some View {
        WorkbenchSettingsCard {
            VStack(spacing: 0) {
                storageRow("Network cache", bytes: storage?.networkBytes)
                Divider()
                storageRow(
                    "Napplets and settings",
                    bytes: storage?.appBytes
                )
                Divider()
                storageRow(
                    storage?.isEstimate == true ? "Total (at least)" : "Total",
                    bytes: storage?.totalBytes
                )
                Divider()
                HStack(spacing: 16) {
                    settingsRowIcon("trash")
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Clear network cache")
                            .font(.body.weight(.medium))
                        Text("Accounts, napplets, and settings stay.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("Clear…", role: .destructive) {
                        requestCacheClear()
                    }
                    .disabled(storage == nil || isBusy)
                    .accessibilityIdentifier("settings-clear-network-cache")
                }
                .padding(.vertical, 12)
            }
        }
    }

    @ViewBuilder
    private var changesFooter: some View {
        if hasChanges {
            HStack(spacing: 10) {
                Spacer()
                Button("Revert", action: discardChanges)
                    .disabled(isBusy)
                Button {
                    applyChanges()
                } label: {
                    if isBusy {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityLabel("Applying changes")
                    } else {
                        Text("Apply")
                    }
                }
                .buttonStyle(.borderedProminent)
                .disabled(isBusy)
                .accessibilityIdentifier("settings-save")
            }
            .padding(.top, 2)
        }
    }

    private func storageRow(_ title: String, bytes: UInt64?) -> some View {
        HStack {
            Text(title)
            Spacer()
            Text(bytes.map(Self.formattedBytes) ?? "Unavailable")
                .foregroundStyle(.secondary)
                .monospacedDigit()
        }
        .padding(.vertical, 12)
    }

    private func settingsRowIcon(_ systemImage: String) -> some View {
        Image(systemName: systemImage)
            .font(.system(size: 17, weight: .medium))
            .foregroundStyle(.secondary)
            .frame(width: 24)
            .accessibilityHidden(true)
    }

    private func settingsNotice(_ message: String) -> some View {
        Label(message, systemImage: "exclamationmark.circle")
            .font(.callout)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(14)
            .background(.regularMaterial, in: .rect(cornerRadius: 12))
            .accessibilityIdentifier("settings-error")
    }

    private static func formattedBytes(_ bytes: UInt64) -> String {
        ByteCountFormatter.string(
            fromByteCount: Int64(clamping: bytes),
            countStyle: .file
        )
    }
}

private struct WorkbenchSettingsCard<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        content
            .padding(.horizontal, 18)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                Color.primary.opacity(0.055),
                in: .rect(cornerRadius: 14)
            )
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(Color.primary.opacity(0.05), lineWidth: 0.5)
            }
    }
}
