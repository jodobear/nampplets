import Foundation
import SwiftUI

public enum WorkbenchSettingsDestination: Hashable, Sendable {
    case account
    case installedLibrary
    case activity

    var title: String {
        switch self {
        case .account:
            "Account"
        case .installedLibrary:
            "Installed Library"
        case .activity:
            "Runtime Activity"
        }
    }

    var systemImage: String {
        switch self {
        case .account:
            "person.crop.circle"
        case .installedLibrary:
            "shippingbox"
        case .activity:
            "waveform.path.ecg"
        }
    }

    var detail: String {
        switch self {
        case .account:
            "Register, activate, sign out, or remove local signing accounts."
        case .installedLibrary:
            "Inspect exact installed builds, sessions, and workspace assignments."
        case .activity:
            "Inspect bounded activity and refusal evidence for the selected build."
        }
    }

    var accessibilityIdentifier: String {
        switch self {
        case .account:
            "settings-account"
        case .installedLibrary:
            "settings-installed-library"
        case .activity:
            "settings-activity"
        }
    }
}

/// One-slot handoff between the Settings sheet and its parent presentation
/// owner. A destination is consumed only after Settings is dismissed, which
/// prevents nested-sheet presentation races without introducing a queue.
struct WorkbenchSettingsRouteState: Equatable, Sendable {
    private(set) var pendingDestination: WorkbenchSettingsDestination?

    mutating func schedule(_ destination: WorkbenchSettingsDestination) {
        pendingDestination = destination
    }

    mutating func consumeAfterDismiss(
        settingsIsPresented: Bool
    ) -> WorkbenchSettingsDestination? {
        guard !settingsIsPresented, let pendingDestination else {
            return nil
        }
        self.pendingDestination = nil
        return pendingDestination
    }
}

public enum WorkbenchRuntimeProfileStatus: Equatable, Sendable {
    case open
    case unavailable(reason: String)

    var title: String {
        switch self {
        case .open:
            "Runtime profile open"
        case .unavailable:
            "Runtime profile unavailable"
        }
    }

    var detail: String {
        switch self {
        case .open:
            "This window shares one application-owned runtime and NMP trust profile."
        case let .unavailable(reason):
            reason
        }
    }

    var systemImage: String {
        switch self {
        case .open:
            "checkmark.shield"
        case .unavailable:
            "exclamationmark.triangle"
        }
    }
}

/// Native settings state that describes ownership without exposing secret
/// material, filesystem paths, or a destructive action that the runtime cannot
/// safely perform yet.
public struct WorkbenchSettingsSnapshot: Equatable, Sendable {
    public static let maximumReasonUTF8Bytes = 16 * 1_024

    public let profileStatus: WorkbenchRuntimeProfileStatus

    public init?(
        profileAvailable: Bool,
        unavailableReason: String? = nil
    ) {
        if profileAvailable {
            profileStatus = .open
            return
        }

        let reason = unavailableReason?.trimmingCharacters(
            in: .whitespacesAndNewlines
        ) ?? ""
        guard
            !reason.isEmpty,
            reason.utf8.count <= Self.maximumReasonUTF8Bytes,
            !reason.unicodeScalars.contains(where: {
                CharacterSet.controlCharacters.contains($0)
                    && $0 != "\n"
                    && $0 != "\t"
            })
        else {
            return nil
        }
        profileStatus = .unavailable(reason: reason)
    }
}

public struct WorkbenchSettingsSheet: View {
    @Environment(\.dismiss) private var dismiss

    private let snapshot: WorkbenchSettingsSnapshot
    private let openDestination: (WorkbenchSettingsDestination) -> Void

    public init(
        snapshot: WorkbenchSettingsSnapshot,
        openDestination: @escaping (WorkbenchSettingsDestination) -> Void
    ) {
        self.snapshot = snapshot
        self.openDestination = openDestination
    }

    public var body: some View {
        NavigationStack {
            Form {
                Section("Runtime profile") {
                    Label {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(snapshot.profileStatus.title)
                                .font(.headline)
                            Text(snapshot.profileStatus.detail)
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    } icon: {
                        Image(systemName: snapshot.profileStatus.systemImage)
                    }
                }

                Section("Manage") {
                    ForEach(
                        [
                            WorkbenchSettingsDestination.account,
                            .installedLibrary,
                            .activity,
                        ],
                        id: \.self
                    ) { destination in
                        Button {
                            openDestination(destination)
                            dismiss()
                        } label: {
                            HStack(spacing: 12) {
                                Image(systemName: destination.systemImage)
                                    .frame(width: 22)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(destination.title)
                                    Text(destination.detail)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                                Image(systemName: "chevron.right")
                                    .foregroundStyle(.tertiary)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier(
                            destination.accessibilityIdentifier
                        )
                    }
                }

                Section("Data ownership") {
                    ownershipRow(
                        title: "Runtime component data",
                        systemImage: "shippingbox.and.arrow.backward",
                        detail:
                            "Installed builds, exact-build grants, component storage, workspaces, and bounded activity facts."
                    )
                    ownershipRow(
                        title: "NMP canonical data",
                        systemImage: "network",
                        detail:
                            "Nostr events, relay evidence, routing, pending writes, and durable receipts remain owned by NMP."
                    )
                    ownershipRow(
                        title: "Account vault",
                        systemImage: "key",
                        detail:
                            "Signing capability material and the selected account are stored separately from both data stores."
                    )
                }

                Section("Reset") {
                    Label {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("Destructive reset is not exposed")
                                .font(.headline)
                            Text(
                                "A safe reset must first close every runtime session and the NMP engine, then let the user choose runtime component data, NMP canonical data, and account-vault material separately."
                            )
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        }
                    } icon: {
                        Image(systemName: "externaldrive.badge.exclamationmark")
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
            }
        }
        #if os(macOS)
        .frame(minWidth: 620, minHeight: 560)
        #endif
    }

    private func ownershipRow(
        title: String,
        systemImage: String,
        detail: String
    ) -> some View {
        Label {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } icon: {
            Image(systemName: systemImage)
        }
    }
}
