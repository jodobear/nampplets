import Foundation
import NMPNativeRuntimeApple
import SwiftUI

public struct ContentView: View {
    private static let workspaceID = "default"

    private let profile: WorkbenchRuntimeProfile?
    private let bootstrapError: String?
    private let layoutStore: any WorkbenchLayoutPersisting
    private let accountManager: any WorkbenchAccountManaging
    private let catalogClient: any CatalogClient
    private let libraryManager: any WorkbenchLibraryManaging

    @State private var selection = "Home"
    @State private var activity = "Opening application runtime profile"
    @State private var artifact: NappletArtifact?
    @State private var composerDraft = ""
    @State private var detailDestination = "No selection"
    @State private var layout: WorkbenchLayoutModel
    @State private var layoutPersistenceError: String?
    @State private var pendingLayoutSave: DispatchWorkItem?
    @State private var isAccountSheetPresented = false
    @State private var isCatalogSheetPresented = false
    @State private var isLibrarySheetPresented = false
    @State private var isActivitySheetPresented = false
    @State private var activitySource: RuntimeWorkbenchActivitySource?
    @State private var activitySheetError: String?
    @FocusState private var focusedRole: WorkbenchSlotRole?

    @MainActor
    public init(
        profile: WorkbenchRuntimeProfile? = nil,
        bootstrapError: String? = nil,
        layoutStore: (any WorkbenchLayoutPersisting)? = nil,
        accountManager: (any WorkbenchAccountManaging)? = nil,
        catalogClient: (any CatalogClient)? = nil,
        libraryManager: (any WorkbenchLibraryManaging)? = nil
    ) {
        self.profile = profile
        self.bootstrapError = bootstrapError
        let resolvedLayoutStore: any WorkbenchLayoutPersisting =
            layoutStore
            ?? profile.map(RuntimeWorkbenchLayoutStore.init(profile:))
            ?? VolatileWorkbenchLayoutStore()
        self.layoutStore = resolvedLayoutStore
        self.accountManager =
            accountManager
            ?? profile.map(RuntimeWorkbenchAccountManager.init(profile:))
            ?? UnavailableWorkbenchAccountManager()
        self.catalogClient = catalogClient ?? RuntimeWorkbenchCatalogClient()
        self.libraryManager =
            libraryManager
            ?? UnavailableWorkbenchLibraryManager()

        do {
            let restored = try resolvedLayoutStore.loadLayout(
                workspaceID: Self.workspaceID
            )
            _layout = State(
                initialValue: WorkbenchLayoutModel(
                    snapshot: restored ?? .workbenchDefault
                )
            )
            _layoutPersistenceError = State(initialValue: nil)
        } catch {
            _layout = State(initialValue: WorkbenchLayoutModel())
            _layoutPersistenceError = State(
                initialValue: "Layout was not restored: \(error.localizedDescription)"
            )
        }
    }

    public var body: some View {
        NavigationSplitView {
            List(selection: $selection) {
                Label("Home", systemImage: "house").tag("Home")
                Label("Messages", systemImage: "bubble.left.and.bubble.right")
                    .tag("Messages")
                Label("Groups", systemImage: "person.3").tag("Groups")
                Label("Streams", systemImage: "play.rectangle").tag("Streams")
                Label("Tools", systemImage: "wrench.and.screwdriver").tag("Tools")
            }
            .navigationTitle("Workbench")
            .navigationSplitViewColumnWidth(min: 160, ideal: 190, max: 260)
        } detail: {
            VStack(spacing: 0) {
                workspaceControlStrip

                WorkbenchWorkspaceView(
                    layout: $layout,
                    focusedRole: $focusedRole,
                    onLayoutChange: scheduleLayoutSave,
                    slotContent: slotContent
                )
                .padding(12)

                activityBar
            }
            .navigationTitle(selection)
        }
        .toolbar {
            ToolbarItemGroup {
                Button(
                    "Account",
                    systemImage: "person.crop.circle"
                ) {
                    isAccountSheetPresented = true
                }
                .keyboardShortcut(",", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Opens account registration, switching, and sign-out"
                )
                Button("Search", systemImage: "magnifyingglass") {
                    isCatalogSheetPresented = true
                }
                .keyboardShortcut("f", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Opens the read-only pinned compatibility catalog"
                )
                Button("Install", systemImage: "shippingbox") {
                    isCatalogSheetPresented = true
                }
                .keyboardShortcut("i", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Opens exact-build review; installation is not connected"
                )
                Button("Library", systemImage: "square.stack.3d.up") {
                    isLibrarySheetPresented = true
                }
                .keyboardShortcut("l", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Opens installed napplets and exact-build lifecycle controls"
                )
                Button("Activity", systemImage: "waveform.path.ecg") {
                    openActivityDrawer()
                }
                .keyboardShortcut("a", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Shows bounded activity for the exact Good Morning build"
                )
                Button("Settings", systemImage: "gearshape") {}
            }
        }
        .sheet(isPresented: $isAccountSheetPresented) {
            WorkbenchAccountSheet(manager: accountManager)
        }
        .sheet(isPresented: $isCatalogSheetPresented) {
            CatalogSheet(client: catalogClient)
        }
        .sheet(isPresented: $isLibrarySheetPresented) {
            WorkbenchLibrarySheet(manager: libraryManager)
        }
        .sheet(isPresented: $isActivitySheetPresented) {
            if
                let activitySource,
                let scope = goodMorningActivityScope
            {
                ActivityDrawer(
                    source: activitySource,
                    scope: scope
                )
            } else {
                NavigationStack {
                    ContentUnavailableView(
                        "Activity unavailable",
                        systemImage: "waveform.path.ecg.rectangle",
                        description: Text(
                            activitySheetError
                                ?? "The exact-build activity source was not admitted."
                        )
                    )
                    .navigationTitle("Runtime Activity")
                    .frame(minWidth: 620, minHeight: 420)
                }
            }
        }
        .task(id: profile.map(ObjectIdentifier.init)) {
            if let bootstrapError {
                activity = "Refused: \(bootstrapError)"
                return
            }
            guard let profile else {
                activity = "Opening application runtime profile"
                return
            }
            profile.native.setIncActionHandler { action in
                Task { @MainActor in
                    handleNativeAction(action)
                }
            }
            do {
                let fixture = try GoodMorningFixture.load()
                artifact = try await Task.detached {
                    try fixture.open(profile: profile)
                }.value
                activity = "Signed exact-build session ready"
            } catch {
                activity = "Refused: \(error.localizedDescription)"
            }
        }
        .onChange(of: focusedRole) { _, newRole in
            guard let newRole, layout.snapshot.focusedRole != newRole else {
                return
            }
            var next = layout
            next.focus(newRole)
            layout = next
            scheduleLayoutSave()
        }
        .onDisappear {
            pendingLayoutSave?.cancel()
            persistLayoutImmediately()
            profile?.native.setIncActionHandler(nil)
        }
        .frame(minWidth: 1_050, minHeight: 660)
    }

    private var workspaceControlStrip: some View {
        HStack(spacing: 12) {
            Label("Workspace", systemImage: "rectangle.split.3x1")
                .font(.headline)

            if let role = role(containing: .goodMorning) {
                Text("Good Morning → \(role.title)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            Button("Show All Slots") {
                mutateLayout { layout in
                    for role in WorkbenchSlotRole.allCases {
                        layout.setVisible(true, role: role)
                    }
                }
            }
            .accessibilityHint("Makes Feed, Detail, Composer, and Tool visible")

            layoutMenu
                .labelsHidden()
        }
        .padding(.horizontal, 14)
        .frame(height: 42)
        .background(.bar)
    }

    private var layoutMenu: some View {
        Menu {
            Section("Show or hide") {
                ForEach(WorkbenchSlotRole.allCases, id: \.self) { role in
                    Button {
                        mutateLayout {
                            $0.setVisible(!$0.isVisible(role), role: role)
                        }
                    } label: {
                        Label(
                            "\(layout.isVisible(role) ? "Hide" : "Show") \(role.title)",
                            systemImage: layout.isVisible(role) ? "eye.slash" : "eye"
                        )
                    }
                    .keyboardShortcut(
                        keyEquivalent(for: role),
                        modifiers: [.command, .shift]
                    )
                }
            }

            Section("Focus") {
                ForEach(WorkbenchSlotRole.allCases, id: \.self) { role in
                    Button {
                        mutateLayout { $0.focus(role) }
                    } label: {
                        Label("Focus \(role.title)", systemImage: role.systemImage)
                    }
                    .keyboardShortcut(
                        keyEquivalent(for: role),
                        modifiers: [.command, .option]
                    )
                }
            }

            Section("Good Morning role") {
                ForEach(WorkbenchSlotRole.allCases, id: \.self) { role in
                    Button {
                        mutateLayout { $0.move(.goodMorning, to: role) }
                    } label: {
                        if layout.component(in: role) == .goodMorning {
                            Label(role.title, systemImage: "checkmark")
                        } else {
                            Text("Move to \(role.title)")
                        }
                    }
                    .keyboardShortcut(
                        keyEquivalent(for: role),
                        modifiers: [.command, .control]
                    )
                }
            }
        } label: {
            Label("Layout", systemImage: "rectangle.split.3x1")
        }
        .accessibilityHint(
            "Shows, hides, focuses, or assigns Good Morning to workspace slots"
        )
    }

    private var activityBar: some View {
        HStack(spacing: 8) {
            Image(systemName: activitySymbol)
                .foregroundStyle(activityColor)
            Text(activity)
            if let layoutPersistenceError {
                Divider()
                    .frame(height: 16)
                Label(layoutPersistenceError, systemImage: "externaldrive.badge.exclamationmark")
                    .foregroundStyle(.orange)
            }
            Spacer()
            Text("Direct napplet network denied · ephemeral WebKit store")
                .foregroundStyle(.secondary)
        }
        .font(.caption)
        .padding(.horizontal, 16)
        .frame(height: 34)
        .background(.bar)
        .accessibilityIdentifier("runtime-activity")
    }

    @ViewBuilder
    private func slotContent(_ role: WorkbenchSlotRole) -> some View {
        if layout.component(in: role) == .goodMorning {
            nappletSurface
        } else {
            switch role {
            case .feed:
                ContentUnavailableView(
                    "No feed renderer",
                    systemImage: "rectangle.stack.badge.minus",
                    description: Text("Assign Good Morning or choose an installed renderer.")
                )
            case .detail:
                ContentUnavailableView(
                    detailDestination,
                    systemImage: "doc.text.magnifyingglass",
                    description: Text("Events and profiles open here.")
                )
            case .composer:
                VStack(alignment: .leading, spacing: 8) {
                    TextEditor(text: $composerDraft)
                        .font(.body)
                        .scrollContentBackground(.hidden)
                        .padding(6)
                        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 6))
                        .accessibilityLabel("Native composer draft")
                    HStack {
                        Text("Native draft · no write is sent without approval")
                            .foregroundStyle(.secondary)
                        Spacer()
                        Button("Review Draft") {}
                            .disabled(composerDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    .font(.caption)
                }
                .padding(10)
            case .tool:
                ContentUnavailableView(
                    "No tool assigned",
                    systemImage: "wrench.and.screwdriver",
                    description: Text("Move Good Morning here or choose an installed tool.")
                )
            }
        }
    }

    @ViewBuilder
    private var nappletSurface: some View {
        if let artifact {
            TrustedNappletView(artifact: artifact) { event in
                switch event {
                case .loading:
                    activity = "Loading trusted shell"
                case .mounted:
                    activity = "Signed Good Morning napplet mounted"
                case .request(let type):
                    activity = "Mapped \(type) from napplet window"
                case .refused(let reason):
                    activity = "Refused: \(reason)"
                case .crashed:
                    activity = "Napplet WebView crashed"
                }
            }
            .accessibilityIdentifier("bundled-napplet")
        } else {
            ProgressView("Loading verified artifact…")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private var activitySymbol: String {
        activity.hasPrefix("Refused") || activity.contains("crashed")
            ? "exclamationmark.triangle.fill"
            : "checkmark.shield.fill"
    }

    private var activityColor: Color {
        activity.hasPrefix("Refused") || activity.contains("crashed")
            ? .orange
            : .green
    }

    private var goodMorningActivityScope: ActivityExactBuildScope? {
        ActivityExactBuildScope(
            manifestAuthor: GoodMorningFixture.author,
            dTag: GoodMorningFixture.dTag,
            aggregateHash: GoodMorningFixture.aggregateHash
        )
    }

    @MainActor
    private func openActivityDrawer() {
        activitySheetError = nil
        guard let profile else {
            activitySheetError =
                bootstrapError ?? "The application runtime profile is unavailable."
            isActivitySheetPresented = true
            return
        }
        guard let scope = goodMorningActivityScope else {
            activitySheetError =
                "The bundled Good Morning exact-build identity is invalid."
            isActivitySheetPresented = true
            return
        }
        if activitySource == nil {
            do {
                activitySource = try RuntimeWorkbenchActivitySource(
                    profile: profile,
                    scope: scope
                )
            } catch {
                activitySheetError = error.localizedDescription
            }
        }
        isActivitySheetPresented = true
    }

    private func role(
        containing component: WorkbenchComponentID
    ) -> WorkbenchSlotRole? {
        WorkbenchSlotRole.allCases.first {
            layout.component(in: $0) == component
        }
    }

    private func keyEquivalent(for role: WorkbenchSlotRole) -> KeyEquivalent {
        switch role {
        case .feed: "1"
        case .detail: "2"
        case .composer: "3"
        case .tool: "4"
        }
    }

    private func mutateLayout(
        _ mutation: (inout WorkbenchLayoutModel) -> Void
    ) {
        var next = layout
        mutation(&next)
        guard next != layout else {
            return
        }
        layout = next
        focusedRole = next.snapshot.focusedRole
        scheduleLayoutSave()
    }

    private func scheduleLayoutSave() {
        guard pendingLayoutSave == nil else {
            return
        }
        let pending = DispatchWorkItem {
            guard !(pendingLayoutSave?.isCancelled ?? true) else {
                pendingLayoutSave = nil
                return
            }
            pendingLayoutSave = nil
            do {
                try layoutStore.saveLayout(
                    layout.snapshot,
                    workspaceID: Self.workspaceID
                )
                layoutPersistenceError = nil
            } catch {
                layoutPersistenceError =
                    "Layout was not saved: \(error.localizedDescription)"
            }
        }
        pendingLayoutSave = pending
        DispatchQueue.main.async(execute: pending)
    }

    private func persistLayoutImmediately() {
        do {
            try layoutStore.saveLayout(
                layout.snapshot,
                workspaceID: Self.workspaceID
            )
            layoutPersistenceError = nil
        } catch {
            layoutPersistenceError =
                "Layout was not saved: \(error.localizedDescription)"
        }
    }

    @MainActor
    private func handleNativeAction(_ action: NativeWorkbenchAction) {
        let payload = (try? JSONSerialization.jsonObject(
            with: Data(action.payloadJSON.utf8)
        )) as? [String: Any]
        switch action.kind {
        case .noteOpen:
            let target = payload?["target"] as? [String: Any]
            let identifier =
                target?["id"] as? String
                ?? target?["identifier"] as? String
                ?? "note"
            detailDestination = "Note \(identifier.prefix(12))"
            mutateLayout { $0.focus(.detail) }
            activity = "Opened note action from Good Morning"
        case .profileOpen:
            let publicKey = payload?["pubkey"] as? String ?? "profile"
            detailDestination = "Profile \(publicKey.prefix(12))"
            mutateLayout { $0.focus(.detail) }
            activity = "Opened profile action from Good Morning"
        case .composeOpen:
            mutateLayout { $0.focus(.composer) }
            activity = "Opened native reply composer from Good Morning"
        }
    }
}
