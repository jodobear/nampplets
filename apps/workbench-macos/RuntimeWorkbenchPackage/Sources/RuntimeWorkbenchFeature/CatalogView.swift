import SwiftUI

public struct CatalogSheet: View {
    @State private var model: CatalogViewModel
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focus: FocusTarget?

    private enum FocusTarget {
        case search
        case coordinate
    }

    @MainActor
    public init(
        client: any CatalogClient,
        onInstalled: @escaping @MainActor (CatalogInstalledBuild) -> Void = {
            _ in
        }
    ) {
        _model = State(
            initialValue: CatalogViewModel(
                client: client,
                onInstalled: onInstalled
            )
        )
    }

    public var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                searchControls
                Divider()
                results
            }
            .navigationTitle("Napplet Catalog")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") {
                        model.cancelReview()
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                }
            }
        }
        .frame(minWidth: 720, idealWidth: 860, minHeight: 540, idealHeight: 680)
        .sheet(
            item: Binding(
                get: { model.review },
                set: { review in
                    if review == nil {
                        model.cancelReview()
                    }
                }
            )
        ) { review in
            CatalogInstallReviewSheet(
                review: review,
                isInstalling: model.isInstalling,
                issue: model.issue,
                onCancel: model.cancelReview,
                onConfirm: {
                    Task {
                        if await model.confirmInstall() != nil {
                            dismiss()
                        }
                    }
                }
            )
        }
        .onAppear {
            focus = .search
        }
        .task {
            await model.start()
        }
        .onDisappear {
            model.stop()
        }
    }

    private var searchControls: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                TextField("Filter the current catalog window", text: $model.query)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .search)
                    .onSubmit {
                        Task {
                            await model.search()
                        }
                    }
                    .accessibilityLabel("Search napplet catalog")
                    .accessibilityHint(
                        "Filters the current bounded NMP window locally"
                    )

                Button("Search", systemImage: "magnifyingglass") {
                    Task {
                        await model.search()
                    }
                }
                .keyboardShortcut(.return, modifiers: [.command])
            }

            Text(
                "The pinned NMP facade does not expose NIP-50 full-text search. "
                    + "Live queries filter the current finite window locally."
            )
            .font(.caption)
            .foregroundStyle(.secondary)

            HStack {
                TextField(
                    "Manual manifest coordinate",
                    text: $model.manualCoordinate
                )
                .textFieldStyle(.roundedBorder)
                .focused($focus, equals: .coordinate)
                .onSubmit {
                    Task {
                        await model.reviewManualCoordinate()
                    }
                }
                .accessibilityLabel("Manual napplet coordinate")
                .accessibilityHint(
                    "Resolves the coordinate before showing an install review"
                )

                Button("Review Coordinate", systemImage: "doc.text.magnifyingglass") {
                    Task {
                        await model.reviewManualCoordinate()
                    }
                }
                .keyboardShortcut("i", modifiers: [.command])
                .disabled(model.isResolvingReview)
            }

            if model.isResolvingReview {
                ProgressView(
                    "Resolving verified build"
                )
                .controlSize(.small)
                .accessibilityLabel(
                    "Resolving napplet coordinate"
                )
            }

            if let issue = model.issue, model.review == nil {
                CatalogIssueView(issue: issue)
            }
        }
        .padding()
    }

    @ViewBuilder
    private var results: some View {
        VStack(spacing: 0) {
            if let evidence = model.evidence {
                CatalogBrowseEvidenceView(
                    evidence: evidence,
                    hasMore: model.hasMore
                )
                Divider()
            }

            resultRows
        }
    }

    @ViewBuilder
    private var resultRows: some View {
        if model.entries.isEmpty {
            ContentUnavailableView(
                "No napplets in this feed",
                systemImage: "square.grid.2x2",
                description: Text(
                    model.evidence == nil
                        ? "The live catalog is unavailable for this profile."
                        : "The current bounded live replacement has no matching napplets."
                )
            )
        } else {
            List(model.entries) { entry in
                Button {
                    Task {
                        await model.review(entry: entry)
                    }
                } label: {
                    CatalogEntryRow(entry: entry)
                }
                .buttonStyle(.plain)
                .disabled(model.isResolvingReview)
                .accessibilityIdentifier("catalog-entry")
                .accessibilityElement(children: .combine)
                .accessibilityLabel(
                    "\(entry.title), by \(entry.publisher.visibleName), "
                        + "\(entry.compatibility.title)"
                )
                .accessibilityHint("Opens the verified install review")
            }
        }
    }
}

private struct CatalogBrowseEvidenceView: View {
    let evidence: CatalogBrowseEvidence
    let hasMore: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack {
                Label(scopeTitle, systemImage: scopeSymbol)
                    .font(.headline)
                Spacer()
                Text(
                    "\(evidence.projectedRows) candidates"
                )
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
            }

            Text(scopeDetail)
                .font(.caption)
                .foregroundStyle(.secondary)

            if evidence.scope == .liveNMPWindow {
                Text(windowDetail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if evidence.locallyFilteredRows > 0 {
                Text(
                    "\(evidence.locallyFilteredRows) rows were excluded by "
                        + "the local filter."
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }

            if evidence.projectionLimitedRows > 0 {
                Text(
                    "\(evidence.projectionLimitedRows) matching rows were "
                        + "omitted by the bounded screen projection."
                )
                .font(.caption)
                .foregroundStyle(.orange)
            }

            if evidence.refusedRows > 0 {
                Text(
                    "\(evidence.refusedRows) malformed or oversized rows were refused."
                )
                .font(.caption)
                .foregroundStyle(.orange)
            }

            if hasMore {
                Label(
                    "More rows exist outside this projection; refine the local filter.",
                    systemImage: "ellipsis.circle"
                )
                .font(.caption)
                .foregroundStyle(.orange)
            }

            if !evidence.shortfalls.isEmpty {
                Text(evidence.shortfalls.map(shortfallTitle).joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.orange)
            }

            if !evidence.sourceEvidence.isEmpty {
                HStack(spacing: 12) {
                    ForEach(evidence.sourceEvidence.prefix(3)) { source in
                        Label(
                            "\(source.source) · \(accessTitle(source.access))",
                            systemImage: sourceSymbol(source.status)
                        )
                        .font(.caption)
                        .foregroundStyle(sourceColor(source.status))
                    }
                    if evidence.sourceEvidence.count > 3 {
                        Text("+\(evidence.sourceEvidence.count - 3) sources")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                .lineLimit(1)
                .help("Source-scoped evidence from the current NMP observation")
            }
        }
        .padding(.horizontal)
        .padding(.vertical, 10)
        .background(.bar)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "\(scopeTitle) · \(evidence.projectedRows) candidates · \(scopeDetail)"
        )
        .accessibilityValue(
            "\(scopeTitle) · \(evidence.projectedRows) candidates · \(scopeDetail)"
        )
        .accessibilityIdentifier("catalog-feed-evidence")
    }

    private var scopeTitle: String {
        switch evidence.scope {
        case .liveNMPWindow:
            "Live NMP catalog window"
        case .offlineFixture:
            "Offline UI-test catalog"
        }
    }

    private var scopeSymbol: String {
        switch evidence.scope {
        case .liveNMPWindow:
            "network"
        case .offlineFixture:
            "testtube.2"
        }
    }

    private var scopeDetail: String {
        switch evidence.scope {
        case .liveNMPWindow:
            "Source-scoped evidence only; this is not a globally complete network result."
        case .offlineFixture:
            "Deterministic bundled compatibility data; no network lookup is performed."
        }
    }

    private var windowDetail: String {
        switch evidence.window {
        case .idle:
            "The NMP window is idle."
        case .requesting:
            "The NMP window is requesting more rows."
        case let .returned(addedRows):
            "The NMP window added \(addedRows) rows."
        case let .atBound(maximumRows):
            "The NMP window reached its \(maximumRows)-row bound."
        case .unknown:
            "The NMP facade did not classify this bounded window state."
        }
    }

    private func shortfallTitle(_ shortfall: CatalogBrowseShortfall) -> String {
        switch shortfall {
        case .noPlannedSource:
            "No planned source"
        case .noResolvedDemand:
            "No resolved demand"
        case .localLimit:
            "Local limit reached"
        }
    }

    private func sourceSymbol(_ status: CatalogBrowseSourceStatus) -> String {
        switch status {
        case .requesting, .connecting:
            "arrow.trianglehead.2.clockwise"
        case .disconnected:
            "bolt.slash"
        case .awaitingAuthentication:
            "person.badge.clock"
        case .authenticationDenied:
            "person.badge.minus"
        case .error:
            "exclamationmark.triangle"
        }
    }

    private func accessTitle(_ access: CatalogBrowseAccessContext) -> String {
        switch access {
        case .public:
            "public"
        case .nip42:
            "NIP-42"
        }
    }

    private func sourceColor(_ status: CatalogBrowseSourceStatus) -> Color {
        switch status {
        case .requesting, .connecting, .awaitingAuthentication:
            .secondary
        case .disconnected, .authenticationDenied, .error:
            .orange
        }
    }
}

private struct CatalogEntryRow: View {
    let entry: CatalogEntry

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: compatibilitySymbol)
                .foregroundStyle(compatibilityColor)
                .frame(width: 24)

            VStack(alignment: .leading, spacing: 4) {
                Text(entry.title)
                    .font(.headline)
                Text(entry.summary)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                LabeledContent("Publisher", value: entry.publisher.visibleName)
                    .font(.caption)
                Text(entry.coordinate)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }

            Spacer()

            Text(entry.compatibility.title)
                .font(.caption)
                .foregroundStyle(compatibilityColor)
        }
        .padding(.vertical, 6)
    }

    private var compatibilitySymbol: String {
        switch entry.compatibility {
        case .unreviewed:
            "doc.text.magnifyingglass"
        case .compatible:
            "checkmark.seal"
        case .incompatible:
            "xmark.octagon"
        case .unknown:
            "questionmark.diamond"
        }
    }

    private var compatibilityColor: Color {
        switch entry.compatibility {
        case .unreviewed:
            .secondary
        case .compatible:
            .green
        case .incompatible:
            .red
        case .unknown:
            .orange
        }
    }
}

private struct CatalogInstallReviewSheet: View {
    let review: CatalogInstallReview
    let isInstalling: Bool
    let issue: CatalogIssue?
    let onCancel: () -> Void
    let onConfirm: () -> Void

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    reviewIdentity
                    Divider()
                    sources
                    Divider()
                    capabilities
                    Divider()
                    compatibility
                    Divider()
                    updateRelationship

                    if !review.warnings.isEmpty {
                        Divider()
                        warnings
                    }

                    if let issue {
                        Divider()
                        CatalogIssueView(issue: issue)
                    }

                    Text(
                        "Installing does not launch this napplet or grant any capability."
                    )
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .accessibilityLabel(
                        "Installing does not launch the napplet or grant capabilities"
                    )
                }
                .padding()
            }
            .navigationTitle("Review \(review.title)")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel", action: onCancel)
                        .keyboardShortcut(.cancelAction)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Install Exact Build", action: onConfirm)
                        .keyboardShortcut(.defaultAction)
                        .disabled(!review.canInstall || isInstalling)
                        .accessibilityIdentifier("catalog-install-exact-build")
                        .accessibilityHint(
                            "Installs only the hash shown in this review"
                        )
                }
            }
        }
        .frame(minWidth: 680, idealWidth: 760, minHeight: 560, idealHeight: 720)
        .interactiveDismissDisabled(isInstalling)
    }

    private var reviewIdentity: some View {
        GroupBox("Verified build") {
            VStack(alignment: .leading, spacing: 8) {
                LabeledContent("Publisher", value: review.publisher.visibleName)
                LabeledContent("Public key", value: review.publisher.publicKey)
                LabeledContent("Coordinate", value: review.coordinate)
                LabeledContent("Exact hash", value: review.exactAggregateHash)
            }
            .font(.body)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var sources: some View {
        CatalogReviewSection(title: "Sources and provenance") {
            if review.sources.isEmpty {
                Text("No source provenance was supplied.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(review.sources) { source in
                    VStack(alignment: .leading, spacing: 3) {
                        Text(source.kind.rawValue)
                            .font(.headline)
                        Text(source.source)
                            .font(.body.monospaced())
                            .textSelection(.enabled)
                        Text(source.evidence)
                            .foregroundStyle(.secondary)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }

    private var capabilities: some View {
        CatalogReviewSection(title: "Capabilities") {
            domainGroup(title: "Required", domains: review.requiredDomains)
            domainGroup(title: "Optional", domains: review.optionalDomains)
        }
    }

    private func domainGroup(title: String, domains: [String]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.headline)
            if domains.isEmpty {
                Text("None")
                    .foregroundStyle(.secondary)
            } else {
                Text(domains.joined(separator: ", "))
                    .textSelection(.enabled)
            }
        }
    }

    private var compatibility: some View {
        CatalogReviewSection(title: "Platform compatibility") {
            if review.platformCompatibility.isEmpty {
                Text("No platform compatibility evidence was projected.")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(review.platformCompatibility) { platform in
                    HStack(alignment: .firstTextBaseline) {
                        Image(systemName: platformSymbol(platform.status))
                            .foregroundStyle(platformColor(platform.status))
                        Text(platform.platform)
                            .font(.headline)
                        Text(platform.detail)
                            .foregroundStyle(.secondary)
                        Spacer()
                    }
                    .accessibilityElement(children: .combine)
                }
            }
        }
    }

    private var updateRelationship: some View {
        CatalogReviewSection(title: "Install relationship") {
            Text(review.updateRelationship.title)
                .font(.headline)
            if let detail = review.updateRelationship.detail {
                Text(detail)
                    .foregroundStyle(.secondary)
            }
            if let installedHash = review.updateRelationship.installedHash {
                LabeledContent("Installed hash", value: installedHash)
                    .textSelection(.enabled)
            }
        }
    }

    private var warnings: some View {
        CatalogReviewSection(title: "Warnings") {
            ForEach(review.warnings) { warning in
                Label(warning.message, systemImage: warningSymbol(warning.severity))
                    .foregroundStyle(warningColor(warning.severity))
            }
        }
    }

    private func platformSymbol(_ status: CatalogPlatformStatus) -> String {
        switch status {
        case .compatible:
            "checkmark.circle"
        case .incompatible:
            "xmark.circle"
        case .unavailable:
            "questionmark.circle"
        }
    }

    private func platformColor(_ status: CatalogPlatformStatus) -> Color {
        switch status {
        case .compatible:
            .green
        case .incompatible:
            .red
        case .unavailable:
            .orange
        }
    }

    private func warningSymbol(_ severity: CatalogWarningSeverity) -> String {
        switch severity {
        case .information:
            "info.circle"
        case .caution:
            "exclamationmark.triangle"
        case .blocking:
            "xmark.octagon"
        }
    }

    private func warningColor(_ severity: CatalogWarningSeverity) -> Color {
        switch severity {
        case .information:
            .secondary
        case .caution:
            .orange
        case .blocking:
            .red
        }
    }
}

private struct CatalogReviewSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title)
                .font(.title3.bold())
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct CatalogIssueView: View {
    let issue: CatalogIssue

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(issue.title)
                    .font(.headline)
                Text(issue.message)
            }
        } icon: {
            Image(systemName: "exclamationmark.triangle")
        }
        .foregroundStyle(.orange)
        .accessibilityElement(children: .combine)
    }
}
