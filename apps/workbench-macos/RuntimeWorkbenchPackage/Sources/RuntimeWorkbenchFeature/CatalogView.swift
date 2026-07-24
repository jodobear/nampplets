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
    public init(client: any CatalogClient) {
        _model = State(initialValue: CatalogViewModel(client: client))
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
                        await model.confirmInstall()
                    }
                }
            )
        }
        .onAppear {
            focus = .search
        }
    }

    private var searchControls: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                TextField("Search approved catalog sources", text: $model.query)
                    .textFieldStyle(.roundedBorder)
                    .focused($focus, equals: .search)
                    .onSubmit {
                        Task {
                            await model.search()
                        }
                    }
                    .accessibilityLabel("Search napplet catalog")
                    .accessibilityHint(
                        "Searches only catalog sources approved by the runtime"
                    )

                Button("Search", systemImage: "magnifyingglass") {
                    Task {
                        await model.search()
                    }
                }
                .keyboardShortcut(.return, modifiers: [.command])
                .disabled(model.isSearching)
            }

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

            if model.isSearching || model.isResolvingReview {
                ProgressView(
                    model.isSearching
                        ? "Searching approved sources"
                        : "Resolving verified build"
                )
                .controlSize(.small)
                .accessibilityLabel(
                    model.isSearching
                        ? "Searching napplet catalog"
                        : "Resolving napplet coordinate"
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
        if model.entries.isEmpty {
            ContentUnavailableView(
                "No catalog results",
                systemImage: "square.grid.2x2",
                description: Text(
                    "Search approved sources or enter a manifest coordinate."
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
                .accessibilityElement(children: .combine)
                .accessibilityLabel(
                    "\(entry.title), by \(entry.publisher.visibleName), "
                        + "\(entry.compatibility.title)"
                )
                .accessibilityHint("Opens the verified install review")
            }
            .overlay(alignment: .bottom) {
                if model.hasMore {
                    Text("More results are available. Refine the search.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(8)
                        .background(.bar, in: Capsule())
                        .padding()
                        .accessibilityLabel(
                            "More results are available; refine the search"
                        )
                }
            }
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

    private var updateRelationship: some View {
        CatalogReviewSection(title: "Install relationship") {
            Text(review.updateRelationship.title)
                .font(.headline)
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
