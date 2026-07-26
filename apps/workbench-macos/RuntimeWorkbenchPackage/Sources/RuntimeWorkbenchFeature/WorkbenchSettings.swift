import SwiftUI

public enum WorkbenchAppearance: String, CaseIterable, Identifiable, Sendable {
    case system
    case light
    case dark

    public static let storageKey = "workbench.appearance"

    public var id: String { rawValue }

    public var title: String {
        switch self {
        case .system: "Automatic"
        case .light: "Light"
        case .dark: "Dark"
        }
    }

    public var colorScheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

public struct WorkbenchSettingsView: View {
    @AppStorage(WorkbenchAppearance.storageKey)
    private var appearance = WorkbenchAppearance.system

    private let snapshot: WorkbenchSettingsSnapshot?
    private let performAction: WorkbenchProfileActionHandler

    @State private var selection = WorkbenchSettingsSection.general
    @State private var draft: WorkbenchProfilePreferences
    @State private var savedPreferences: WorkbenchProfilePreferences?
    @State private var actionError: String?
    @State private var isSaving = false
    @State private var isClearing = false
    @State private var showsClearConfirmation = false

    public init(
        snapshot: WorkbenchSettingsSnapshot?,
        performAction: @escaping WorkbenchProfileActionHandler
    ) {
        self.snapshot = snapshot
        self.performAction = performAction
        let preferences =
            snapshot?.preferences
            ?? WorkbenchProfilePreferences(
                appRelays: [],
                indexerRelays: [],
                permissionDefault: .askEveryTime
            )
        _draft = State(initialValue: preferences)
        _savedPreferences = State(initialValue: snapshot?.preferences)
    }

    public var body: some View {
        NavigationSplitView {
            WorkbenchSettingsSidebar(selection: $selection)
        } detail: {
            WorkbenchSettingsDetail(
                section: selection,
                appearance: $appearance,
                draft: $draft,
                storage: snapshot?.storage,
                unavailableReason: unavailableReason,
                actionError: actionError,
                isBusy: isBusy,
                hasChanges: hasChanges,
                applyChanges: save,
                discardChanges: discardChanges,
                requestCacheClear: {
                    showsClearConfirmation = true
                }
            )
        }
        .navigationSplitViewStyle(.balanced)
        .alert(
            "Clear Network Cache?",
            isPresented: $showsClearConfirmation
        ) {
            Button("Cancel", role: .cancel) {}
            Button("Clear Cache", role: .destructive) {
                clearNetworkCache()
            }
        } message: {
            Text(
                "This removes cached network activity and delivery history, "
                    + "including anything still waiting to send. Accounts, "
                    + "napplets, permissions, and settings stay."
            )
        }
        #if os(macOS)
        .frame(minWidth: 880, idealWidth: 980, minHeight: 600, idealHeight: 680)
        #endif
    }

    private var unavailableReason: String? {
        guard let snapshot else {
            return "Settings are unavailable while the app is opening."
        }
        if case let .unavailable(reason) = snapshot.profileStatus {
            return reason
        }
        return nil
    }

    private var isBusy: Bool {
        isSaving || isClearing
    }

    private var hasChanges: Bool {
        !isBusy && savedPreferences != nil && draft != savedPreferences
    }

    private func discardChanges() {
        guard let savedPreferences else {
            return
        }
        draft = savedPreferences
        actionError = nil
    }

    private func save() {
        let normalized: WorkbenchProfilePreferences
        do {
            normalized = try draft.normalized()
        } catch {
            actionError = error.localizedDescription
            return
        }
        isSaving = true
        actionError = nil
        Task { @MainActor in
            do {
                try await performAction(.savePreferences(normalized))
                draft = normalized
                savedPreferences = normalized
                isSaving = false
            } catch {
                actionError = error.localizedDescription
                isSaving = false
            }
        }
    }

    private func clearNetworkCache() {
        isClearing = true
        actionError = nil
        Task { @MainActor in
            do {
                try await performAction(.clearNetworkCache)
                isClearing = false
            } catch {
                actionError = error.localizedDescription
                isClearing = false
            }
        }
    }
}
