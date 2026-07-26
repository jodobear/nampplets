import SwiftUI

/// Discover: the front door, built as a page rather than a shelf.
///
/// This product's content is words and names -- there is no artwork, no
/// rating, no count, no chart and no editorial. A visual language of tiles and
/// hero banners would be spending its budget on a payload we do not have, so
/// the genre here is the well-set printed page: one measure, leading-aligned,
/// hierarchy from size and space rather than from boxes, warm ground, serif
/// for the things a person named.
///
/// It looks the same with three napplets as with thirty. There is no sparse
/// variant, no getting-started module and no reduced-inventory apology,
/// because a single column of full-measure cards is already the right shape
/// for three -- so three looks like the design working rather than like a
/// failure.
///
/// See `docs/design/napplet-browser-visual.md`.
public struct CatalogSheet: View {
    @State var model: CatalogViewModel
    @State var isShowingAddress = false
    @State private var pressedEntryID: String?
    @State private var availableWidth = 0.0
    @Environment(\.dismiss) var dismiss
    @FocusState var focus: FocusTarget?

    enum FocusTarget {
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
        VStack(spacing: 0) {
            page
            footer
        }
        .background(NappletInk.paper)
        #if os(macOS)
        .frame(
            // Wide enough that the two-column grid actually engages: a sheet
            // sized to its minimum was rendering a single column of cards in
            // a tall thin strip, which is the shape of a list, not a store.
            minWidth: 940,
            idealWidth: 1_040,
            minHeight: 620,
            idealHeight: 820
        )
        #endif
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
                issuePresentation: model.installIssuePresentation,
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
        .onAppear { focus = .search }
        .task { await model.start() }
        .onDisappear { model.stop() }
    }

    private var page: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 0) {
                searchField
                    .padding(.bottom, NappletMetrics.generous)

                Text("Napplets")
                    .font(NappletType.place)
                    .nappletDisplayFace()
                    .foregroundStyle(NappletInk.ink)

                Text(lede)
                    .font(NappletType.lede)
                    .foregroundStyle(NappletInk.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, NappletMetrics.tight)

                if let presentedIssue = model.presentedIssue, model.review == nil {
                    CatalogIssueNotice(presentation: presentedIssue)
                        .padding(.top, NappletMetrics.roomy)
                }

                content
                    .padding(.top, NappletMetrics.spacious)
            }
            .frame(maxWidth: contentWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, NappletMetrics.generous)
            .padding(.top, NappletMetrics.generous)
            .padding(.bottom, NappletMetrics.spacious)
        }
        .background {
            GeometryReader { proxy in
                Color.clear
                    .onAppear { availableWidth = proxy.size.width }
                    .onChange(of: proxy.size.width) { _, width in
                        availableWidth = width
                    }
            }
        }
    }

    /// Prose stays inside one measure; a two-column grid is allowed the width
    /// of two measures, because each card is its own column of text.
    private var contentWidth: Double {
        columns.count > 1
            ? NappletMetrics.measure * 2 + NappletMetrics.snug
            : NappletMetrics.measure
    }

    private var lede: String {
        model.entries.isEmpty
            ? "Small apps, signed by whoever made them."
            : "From the latest lookup on this device."
    }

    private var searchField: some View {
        HStack(spacing: NappletMetrics.tight) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(NappletInk.inkSecondary)
                .accessibilityHidden(true)
            TextField("Filter these napplets", text: $model.query)
                .textFieldStyle(.plain)
                .font(NappletType.body)
                .focused($focus, equals: .search)
                .onSubmit { Task { await model.search() } }
                .accessibilityLabel("Filter these napplets")
                .accessibilityHint("Filters the napplets shown here")
            if !model.query.isEmpty {
                Button {
                    model.query = ""
                    Task { await model.search() }
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(NappletInk.inkSecondary)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Clear search")
            }
        }
        .padding(.horizontal, NappletMetrics.snug)
        .padding(.vertical, NappletMetrics.tight + 2)
        .background(
            NappletInk.fillQuiet,
            in: RoundedRectangle(cornerRadius: NappletMetrics.tight, style: .continuous)
        )
    }

    @ViewBuilder
    private var content: some View {
        if model.entries.isEmpty, model.evidence?.window == .requesting {
            waiting
        } else if model.entries.isEmpty {
            empty
        } else {
            listing
        }
    }

    private var listing: some View {
        VStack(alignment: .leading, spacing: NappletMetrics.snug) {
            if model.isResolvingReview {
                Text("Checking…")
                    .font(NappletType.caption)
                    .foregroundStyle(NappletInk.inkSecondary)
            }
            // Two columns once there is enough of both -- width and content --
            // for a second column to be a grid rather than a gap. Below either
            // threshold a single column of full-measure cards is the right
            // shape, which is why three napplets need no special layout.
            LazyVGrid(columns: columns, spacing: NappletMetrics.snug) {
                ForEach(model.entries) { entry in
                    Button {
                        Task { await model.review(entry: entry) }
                    } label: {
                        CatalogEntryRow(
                            entry: entry,
                            isPressed: pressedEntryID == entry.id
                        )
                    }
                    .buttonStyle(.plain)
                    .disabled(model.isResolvingReview)
                    .onHover { isInside in
                        pressedEntryID = isInside ? entry.id : nil
                    }
                    .accessibilityIdentifier("catalog-entry")
                    .accessibilityElement(children: .combine)
                    .accessibilityLabel(accessibilityLabel(for: entry))
                    .accessibilityHint(
                        "Shows what this napplet does before you add it"
                    )
                }
            }

            addressDisclosure
                .padding(.top, NappletMetrics.roomy)
        }
    }

    private var columns: [GridItem] {
        let isWide = availableWidth >= 780
        let count = (isWide && model.entries.count >= 4) ? 2 : 1
        return Array(
            repeating: GridItem(
                .flexible(),
                spacing: NappletMetrics.snug,
                alignment: .top
            ),
            count: count
        )
    }

}
