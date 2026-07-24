import Observation
import SwiftUI

@MainActor
public protocol WorkbenchLibrarySubscription: AnyObject {
    func cancel()
}

/// Injectable boundary for the Rust-owned installed-library projection.
///
/// Implementations immediately push one authoritative snapshot, then bounded
/// replacement updates. Commands are fire-and-observe: they never report
/// operation success to Swift. Filtering, lifecycle legality, exact-build
/// cleanup, workspace validation, persistence, and refusal semantics remain
/// in Rust.
@MainActor
public protocol WorkbenchLibraryManaging: AnyObject {
    func subscribe(
        receive: @escaping @MainActor (WorkbenchLibraryUpdate) -> Void
    ) -> any WorkbenchLibrarySubscription

    func refresh() -> WorkbenchLibrarySnapshot
    func setFilter(_ query: String)
    func suspend(sessionID: UInt64)
    func resume(sessionID: UInt64)
    func assign(
        _ exactBuild: WorkbenchLibraryExactBuild,
        toWorkspaceID workspaceID: String
    )
    func clearAssignment(
        _ exactBuild: WorkbenchLibraryExactBuild,
        fromWorkspaceID workspaceID: String
    )
    func uninstall(_ exactBuild: WorkbenchLibraryExactBuild)
}

/// Truthful fallback used when this runtime build does not expose the typed
/// installed-library projection.
///
/// It publishes one immutable authoritative snapshot and never accepts
/// commands. The sheet therefore remains reachable without suggesting that
/// local filtering or lifecycle mutations succeeded.
@MainActor
public final class UnavailableWorkbenchLibraryManager:
    WorkbenchLibraryManaging
{
    public static let defaultReason =
        "Installed-library APIs are unavailable in this runtime build."

    private let snapshot: WorkbenchLibrarySnapshot

    public init(reason: String = defaultReason) {
        let requestedSnapshot = WorkbenchLibrarySnapshot(
            revision: 0,
            availability: .unavailable(reason: reason),
            filterQuery: "",
            totalInstalled: 0,
            builds: [],
            workspaces: []
        )
        guard
            let snapshot = requestedSnapshot
            ?? WorkbenchLibrarySnapshot(
                revision: 0,
                availability: .unavailable(reason: Self.defaultReason),
                filterQuery: "",
                totalInstalled: 0,
                builds: [],
                workspaces: []
            )
        else {
            preconditionFailure(
                "The fixed unavailable library snapshot must remain valid"
            )
        }
        self.snapshot = snapshot
    }

    public func subscribe(
        receive: @escaping @MainActor (WorkbenchLibraryUpdate) -> Void
    ) -> any WorkbenchLibrarySubscription {
        receive(.authoritative(snapshot))
        return UnavailableWorkbenchLibrarySubscription()
    }

    public func refresh() -> WorkbenchLibrarySnapshot {
        snapshot
    }

    public func setFilter(_: String) {}
    public func suspend(sessionID _: UInt64) {}
    public func resume(sessionID _: UInt64) {}

    public func assign(
        _: WorkbenchLibraryExactBuild,
        toWorkspaceID _: String
    ) {}

    public func clearAssignment(
        _: WorkbenchLibraryExactBuild,
        fromWorkspaceID _: String
    ) {}

    public func uninstall(_: WorkbenchLibraryExactBuild) {}
}

@MainActor
private final class UnavailableWorkbenchLibrarySubscription:
    WorkbenchLibrarySubscription
{
    func cancel() {}
}

@MainActor
@Observable
final class WorkbenchLibrarySheetModel {
    var filterDraft = ""

    private(set) var snapshot: WorkbenchLibrarySnapshot?
    private(set) var updateGap: WorkbenchLibraryUpdateGap?

    private let manager: any WorkbenchLibraryManaging
    private var subscription: (any WorkbenchLibrarySubscription)?

    init(manager: any WorkbenchLibraryManaging) {
        self.manager = manager
    }

    var commandsAvailable: Bool {
        snapshot?.availability.isAvailable == true
    }

    func start() {
        guard subscription == nil else {
            return
        }
        subscription = manager.subscribe { [weak self] update in
            self?.receive(update)
        }
    }

    func stop() {
        subscription?.cancel()
        subscription = nil
    }

    func applyFilter() {
        guard commandsAvailable else {
            return
        }
        manager.setFilter(filterDraft)
    }

    func clearFilter() {
        filterDraft = ""
        applyFilter()
    }

    func refresh() {
        receive(.authoritative(manager.refresh()))
    }

    func suspend(_ session: WorkbenchLibrarySession) {
        guard commandsAvailable, session.state == .running else {
            return
        }
        manager.suspend(sessionID: session.id)
    }

    func resume(_ session: WorkbenchLibrarySession) {
        guard commandsAvailable, session.state == .suspended else {
            return
        }
        manager.resume(sessionID: session.id)
    }

    func assign(
        _ exactBuild: WorkbenchLibraryExactBuild,
        to workspace: WorkbenchLibraryWorkspace
    ) {
        guard commandsAvailable else {
            return
        }
        manager.assign(exactBuild, toWorkspaceID: workspace.id)
    }

    func clearAssignment(
        _ exactBuild: WorkbenchLibraryExactBuild,
        from workspace: WorkbenchLibraryWorkspace
    ) {
        guard commandsAvailable else {
            return
        }
        manager.clearAssignment(exactBuild, fromWorkspaceID: workspace.id)
    }

    func uninstall(_ exactBuild: WorkbenchLibraryExactBuild) {
        guard commandsAvailable else {
            return
        }
        manager.uninstall(exactBuild)
    }

    private func receive(_ update: WorkbenchLibraryUpdate) {
        switch update {
        case let .authoritative(nextSnapshot):
            snapshot = nextSnapshot
            filterDraft = nextSnapshot.filterQuery
            updateGap = nil

        case let .next(nextSnapshot, predecessorRevision):
            guard let currentRevision = snapshot?.revision else {
                snapshot = nextSnapshot
                filterDraft = nextSnapshot.filterQuery
                updateGap = WorkbenchLibraryUpdateGap(
                    expectedPredecessorRevision: 0,
                    receivedPredecessorRevision: predecessorRevision,
                    receivedRevision: nextSnapshot.revision
                )
                return
            }
            guard nextSnapshot.revision > currentRevision else {
                return
            }
            if predecessorRevision != currentRevision {
                updateGap = WorkbenchLibraryUpdateGap(
                    expectedPredecessorRevision: currentRevision,
                    receivedPredecessorRevision: predecessorRevision,
                    receivedRevision: nextSnapshot.revision
                )
            }
            snapshot = nextSnapshot
            filterDraft = nextSnapshot.filterQuery
        }
    }
}

public struct WorkbenchLibrarySheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var model: WorkbenchLibrarySheetModel
    @State private var uninstallCandidate: WorkbenchLibraryBuild?
    @FocusState private var filterFocused: Bool
    private let onOpen: @MainActor (WorkbenchLibraryBuild) -> Void

    @MainActor
    public init(
        manager: any WorkbenchLibraryManaging,
        onOpen: @escaping @MainActor (WorkbenchLibraryBuild) -> Void = { _ in }
    ) {
        _model = State(
            initialValue: WorkbenchLibrarySheetModel(manager: manager)
        )
        self.onOpen = onOpen
    }

    public var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                filterBar
                Divider()

                if let snapshot = model.snapshot {
                    if let reason = snapshot.availability.unavailableReason {
                        unavailableBanner(reason)
                        Divider()
                    }

                    if let gap = model.updateGap {
                        updateGapBanner(gap)
                        Divider()
                    }

                    if let refusal = snapshot.refusals.last {
                        refusalBanner(refusal)
                        Divider()
                    }

                    library(snapshot)
                } else {
                    ProgressView("Waiting for installed library…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityLabel(
                            "Waiting for the installed library snapshot"
                        )
                }
            }
            .navigationTitle("Installed Napplets")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") {
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                }

                ToolbarItem {
                    Button("Refresh", systemImage: "arrow.clockwise") {
                        model.refresh()
                    }
                    .accessibilityHint(
                        "Requests one authoritative installed library snapshot"
                    )
                }
            }
        }
        .frame(minWidth: 760, idealWidth: 900, minHeight: 540, idealHeight: 700)
        .onAppear {
            model.start()
            filterFocused = true
        }
        .onDisappear {
            model.stop()
        }
        .confirmationDialog(
            uninstallCandidate.map { "Uninstall \($0.title)?" } ?? "Uninstall exact build?",
            isPresented: Binding(
                get: { uninstallCandidate != nil },
                set: { isPresented in
                    if !isPresented {
                        uninstallCandidate = nil
                    }
                }
            ),
            titleVisibility: .visible,
            presenting: uninstallCandidate
        ) { build in
            Button("Uninstall Exact Build", role: .destructive) {
                model.uninstall(build.exactBuild)
                uninstallCandidate = nil
            }
            Button("Cancel", role: .cancel) {
                uninstallCandidate = nil
            }
        } message: { build in
            Text(
                "This asks the runtime to remove only state owned for "
                    + "\(build.exactBuild.dTag) at aggregate "
                    + "\(build.exactBuild.aggregateHash). The row remains "
                    + "visible until Rust confirms the new library snapshot."
            )
        }
        .accessibilityIdentifier("workbench-installed-library")
    }

    private var filterBar: some View {
        HStack(spacing: 10) {
            TextField("Filter installed napplets", text: $model.filterDraft)
                .textFieldStyle(.roundedBorder)
                .focused($filterFocused)
                .onSubmit {
                    model.applyFilter()
                }
                .disabled(!model.commandsAvailable)
                .accessibilityLabel("Filter installed napplets")
                .accessibilityHint(
                    "Sends the filter to the runtime; results are not filtered locally"
                )

            Button("Filter", systemImage: "line.3.horizontal.decrease.circle") {
                model.applyFilter()
            }
            .keyboardShortcut("f", modifiers: [.command, .option])
            .disabled(!model.commandsAvailable)

            if model.snapshot?.filterQuery.isEmpty == false {
                Button("Clear") {
                    model.clearFilter()
                }
                .disabled(!model.commandsAvailable)
            }
        }
        .padding()
    }

    @ViewBuilder
    private func library(_ snapshot: WorkbenchLibrarySnapshot) -> some View {
        if snapshot.builds.isEmpty {
            ContentUnavailableView(
                snapshot.filterQuery.isEmpty
                    ? "No Installed Napplets"
                    : "No Matching Napplets",
                systemImage: "square.stack.3d.up.slash",
                description: Text(
                    snapshot.filterQuery.isEmpty
                        ? "Verified installations will appear here."
                        : "The runtime found no installed build for this filter."
                )
            )
        } else {
            List(snapshot.builds) { build in
                WorkbenchLibraryBuildRow(
                    build: build,
                    workspaces: snapshot.workspaces,
                    commandsAvailable: model.commandsAvailable,
                    onOpen: {
                        onOpen(build)
                    },
                    onSuspend: model.suspend,
                    onResume: model.resume,
                    onAssign: { workspace in
                        model.assign(build.exactBuild, to: workspace)
                    },
                    onClearAssignment: { workspace in
                        model.clearAssignment(
                            build.exactBuild,
                            from: workspace
                        )
                    },
                    onRequestUninstall: {
                        uninstallCandidate = build
                    }
                )
            }
            .overlay(alignment: .bottomTrailing) {
                Text(
                    "Showing \(snapshot.builds.count) of "
                        + "\(snapshot.totalInstalled) installed"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(8)
                .background(.bar, in: Capsule())
                .padding()
                .accessibilityLabel(
                    "Showing \(snapshot.builds.count) of "
                        + "\(snapshot.totalInstalled) installed napplets"
                )
            }
        }
    }

    private func unavailableBanner(_ reason: String) -> some View {
        LibraryStatusBanner(
            title: "Installed library unavailable",
            message: reason,
            symbol: "externaldrive.badge.xmark",
            color: .orange,
            accessibilityIdentifier: "workbench-library-unavailable"
        )
    }

    private func refusalBanner(_ refusal: WorkbenchLibraryRefusal) -> some View {
        LibraryStatusBanner(
            title: "Runtime refused an action",
            message: "\(refusal.code): \(refusal.message)",
            symbol: "hand.raised",
            color: .red,
            accessibilityIdentifier: "workbench-library-refusal"
        )
    }

    private func updateGapBanner(
        _ gap: WorkbenchLibraryUpdateGap
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.arrow.triangle.2.circlepath")
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text("Library update may be incomplete")
                    .font(.headline)
                Text(
                    "Expected revision \(gap.expectedPredecessorRevision), "
                        + "received \(gap.receivedPredecessorRevision)."
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            Spacer()
            Button("Refresh") {
                model.refresh()
            }
        }
        .padding()
        .background(.orange.opacity(0.08))
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "Library update may be incomplete. Expected predecessor revision "
                + "\(gap.expectedPredecessorRevision), received "
                + "\(gap.receivedPredecessorRevision)."
        )
        .accessibilityHint(
            "Activate Refresh to request an authoritative snapshot"
        )
        .accessibilityIdentifier("workbench-library-update-gap")
    }
}

private struct WorkbenchLibraryBuildRow: View {
    let build: WorkbenchLibraryBuild
    let workspaces: [WorkbenchLibraryWorkspace]
    let commandsAvailable: Bool
    let onOpen: () -> Void
    let onSuspend: (WorkbenchLibrarySession) -> Void
    let onResume: (WorkbenchLibrarySession) -> Void
    let onAssign: (WorkbenchLibraryWorkspace) -> Void
    let onClearAssignment: (WorkbenchLibraryWorkspace) -> Void
    let onRequestUninstall: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: availabilitySymbol)
                    .foregroundStyle(availabilityColor)
                    .font(.title2)
                    .frame(width: 28)
                    .accessibilityHidden(true)

                VStack(alignment: .leading, spacing: 5) {
                    Text(build.title)
                        .font(.headline)
                    Label(
                        build.availability.title,
                        systemImage: availabilitySymbol
                    )
                    .font(.caption)
                    .foregroundStyle(availabilityColor)
                    Text(build.availability.detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                Spacer()

                Button("Open", systemImage: "rectangle.on.rectangle") {
                    onOpen()
                }
                .disabled(!commandsAvailable)
                .accessibilityLabel("Open \(build.title) on canvas")
                .accessibilityIdentifier("open-installed-napplet")

                Menu("Workspace", systemImage: "rectangle.3.group") {
                    if workspaces.isEmpty {
                        Text("No workspaces available")
                    } else {
                        Section("Assign") {
                            ForEach(unassignedWorkspaces) { workspace in
                                Button(workspace.displayName) {
                                    onAssign(workspace)
                                }
                            }
                            if unassignedWorkspaces.isEmpty {
                                Text("Assigned to every workspace")
                            }
                        }

                        if !assignedWorkspaces.isEmpty {
                            Section("Remove assignment") {
                                ForEach(assignedWorkspaces) { workspace in
                                    Button(workspace.displayName) {
                                        onClearAssignment(workspace)
                                    }
                                }
                            }
                        }
                    }
                }
                .disabled(!commandsAvailable || workspaces.isEmpty)
                .accessibilityHint(
                    "Assigns or removes this exact build from a runtime workspace"
                )

                Button(
                    "Uninstall",
                    systemImage: "trash",
                    role: .destructive,
                    action: onRequestUninstall
                )
                .disabled(!commandsAvailable)
                .accessibilityHint(
                    "Opens a confirmation for this exact aggregate"
                )
            }

            exactBuildIdentity

            if !assignedWorkspaces.isEmpty {
                LabeledContent("Assigned workspaces") {
                    Text(
                        assignedWorkspaces
                            .map(\.displayName)
                            .joined(separator: ", ")
                    )
                }
                .font(.caption)
            }

            sessionList
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .contain)
    }

    private var exactBuildIdentity: some View {
        Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 3) {
            GridRow {
                Text("Publisher")
                    .foregroundStyle(.secondary)
                Text(build.exactBuild.manifestAuthor)
                    .textSelection(.enabled)
            }
            GridRow {
                Text("d-tag")
                    .foregroundStyle(.secondary)
                Text(build.exactBuild.dTag)
                    .textSelection(.enabled)
            }
            GridRow {
                Text("Aggregate")
                    .foregroundStyle(.secondary)
                Text(build.exactBuild.aggregateHash)
                    .textSelection(.enabled)
            }
        }
        .font(.caption.monospaced())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "Exact build \(build.exactBuild.dTag), publisher "
                + "\(build.exactBuild.manifestAuthor), aggregate "
                + "\(build.exactBuild.aggregateHash)"
        )
    }

    @ViewBuilder
    private var sessionList: some View {
        if build.sessions.isEmpty {
            Label("No active sessions", systemImage: "pause.rectangle")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                Text("Sessions")
                    .font(.caption.weight(.semibold))

                ForEach(build.sessions) { session in
                    HStack {
                        Label(
                            "Session \(session.id): \(session.state.title)",
                            systemImage: session.state == .running
                                ? "play.circle"
                                : "pause.circle"
                        )
                        .font(.caption)

                        Spacer()

                        switch session.state {
                        case .running:
                            Button("Suspend") {
                                onSuspend(session)
                            }
                            .accessibilityHint(
                                "Asks Rust to suspend session \(session.id)"
                            )
                        case .suspended:
                            Button("Resume") {
                                onResume(session)
                            }
                            .accessibilityHint(
                                "Asks Rust to resume session \(session.id)"
                            )
                        }
                    }
                    .disabled(!commandsAvailable)
                }
            }
        }
    }

    private var assignedWorkspaces: [WorkbenchLibraryWorkspace] {
        let assignedIDs = Set(build.assignedWorkspaceIDs)
        return workspaces.filter { assignedIDs.contains($0.id) }
    }

    private var unassignedWorkspaces: [WorkbenchLibraryWorkspace] {
        let assignedIDs = Set(build.assignedWorkspaceIDs)
        return workspaces.filter { !assignedIDs.contains($0.id) }
    }

    private var availabilitySymbol: String {
        switch build.availability {
        case .metadataOnly:
            "doc.text.magnifyingglass"
        case .sealedExactBytesReady:
            "checkmark.seal"
        }
    }

    private var availabilityColor: Color {
        switch build.availability {
        case .metadataOnly:
            .orange
        case .sealedExactBytesReady:
            .green
        }
    }
}

private struct LibraryStatusBanner: View {
    let title: String
    let message: String
    let symbol: String
    let color: Color
    let accessibilityIdentifier: String

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol)
                .foregroundStyle(color)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding()
        .background(color.opacity(0.08))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(title). \(message)")
        .accessibilityIdentifier(accessibilityIdentifier)
    }
}
