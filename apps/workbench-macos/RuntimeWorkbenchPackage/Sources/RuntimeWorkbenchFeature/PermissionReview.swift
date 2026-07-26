import Observation
import SwiftUI

/// The Rust-backed permission boundary consumed by the native review sheet.
///
/// `submit` accepts one finite batch for one exact build. Implementations own
/// dependency validation, persistence, revocation, provider cancellation, and
/// the resulting state. This interface deliberately exposes no launch action.
@MainActor
public protocol PermissionReviewManaging: AnyObject {
    func snapshot() -> PermissionReviewSnapshot
    func submit(_ batch: PermissionDecisionBatch) async
}

@MainActor
@Observable
final class PermissionReviewSheetModel {
    private let manager: any PermissionReviewManaging
    private(set) var snapshot: PermissionReviewSnapshot
    private(set) var selections: [String: PermissionRequestedDecision]
    private(set) var transientIssue: PermissionReviewIssue?
    private(set) var isSubmitting = false

    init(manager: any PermissionReviewManaging) {
        self.manager = manager
        let snapshot = manager.snapshot()
        self.snapshot = snapshot
        selections = Dictionary(
            uniqueKeysWithValues: snapshot.review.capabilities.compactMap {
                capability in
                capability.requestedDecision.map {
                    (capability.domain, $0)
                }
            }
        )
    }

    var review: PermissionReview {
        snapshot.review
    }

    var issue: PermissionReviewIssue? {
        if let transientIssue {
            return transientIssue
        }
        guard case let .refused(issue) = snapshot.submissionState else {
            return nil
        }
        return issue
    }

    var isApplied: Bool {
        snapshot.submissionState == .applied
    }

    var canConfirm: Bool {
        !isSubmitting
            && !isApplied
            && transientIssue == nil
            && invalidSelections.isEmpty
    }

    func selection(
        for capability: PermissionCapabilityReview
    ) -> PermissionRequestedDecision? {
        selections[capability.domain] ?? capability.requestedDecision
    }

    func select(
        _ decision: PermissionRequestedDecision,
        for requestedCapability: PermissionCapabilityReview
    ) {
        guard
            let capability = review.capabilities.first(where: {
                $0.domain == requestedCapability.domain
            }),
            let option = capability.option(for: decision),
            option.isValid
        else {
            transientIssue = PermissionReviewIssue(
                title: "Decision unavailable",
                message: requestedCapability.option(for: decision)?.invalidReason
                    ?? "The runtime did not offer this decision.",
                affectedDomains: [requestedCapability.domain]
            )!
            return
        }
        selections[capability.domain] = decision
        transientIssue = nil
    }

    /// Applies the decision Rust itself recommends for this capability, or
    /// denies. This layer never ranks `decisionOptions` on its own:
    /// session-vs-exact-build scope is a runtime policy question, and
    /// `recommendedDecision` is Rust's answer to it.
    func setGranted(_ granted: Bool, for capability: PermissionCapabilityReview) {
        guard granted else {
            if capability.option(for: .deny)?.isValid == true {
                select(.deny, for: capability)
            }
            return
        }
        guard let recommended = grantingRecommendation(for: capability) else {
            return
        }
        select(recommended, for: capability)
    }

    /// Whether this capability currently reads as granted. Before the user
    /// edits anything that is Rust's own `isGranted` classification of the
    /// decision in force; afterwards it is whether the pending selection is
    /// the runtime's recommended grant.
    func isGranted(_ capability: PermissionCapabilityReview) -> Bool {
        guard let selected = selection(for: capability) else {
            return capability.isGranted
        }
        guard selected != capability.requestedDecision else {
            return capability.isGranted
        }
        return selected == grantingRecommendation(for: capability)
    }

    func hasAffirmativeOption(_ capability: PermissionCapabilityReview) -> Bool {
        grantingRecommendation(for: capability) != nil
    }

    /// Rust's recommended decision, but only when that recommendation is an
    /// actual grant. A capability the runtime recommends denying offers the
    /// user nothing to switch on.
    private func grantingRecommendation(
        for capability: PermissionCapabilityReview
    ) -> PermissionRequestedDecision? {
        guard
            let recommended = capability.recommendedDecision,
            recommended != .deny,
            recommended != .askEveryTime,
            capability.option(for: recommended)?.isValid == true
        else {
            return nil
        }
        return recommended
    }

    /// Discards only transient native form state. It never calls the manager.
    func cancel() {
        selections = Dictionary(
            uniqueKeysWithValues: review.capabilities.compactMap {
                capability in
                capability.requestedDecision.map {
                    (capability.domain, $0)
                }
            }
        )
        transientIssue = nil
    }

    /// Selects, for every capability the runtime lets the user decide, the
    /// decision Rust itself recommends -- never one this model invents by
    /// ranking `decisionOptions`. Managed capabilities (no
    /// `requestedDecision`, and therefore no `recommendedDecision`) are
    /// untouched; they carry no user-selectable option at all. This is the
    /// one-tap "just let me try it" path: the Rust-requested default is
    /// `askEveryTime`, which can never satisfy launch, so confirming
    /// un-edited defaults previously looked like it worked and then
    /// silently failed to launch.
    func selectAllRecommended() {
        for capability in review.capabilities {
            guard
                capability.requestedDecision != nil,
                let recommended = capability.recommendedDecision,
                capability.option(for: recommended)?.isValid == true
            else {
                continue
            }
            selections[capability.domain] = recommended
        }
        transientIssue = nil
    }

    func confirm() async {
        guard canConfirm else {
            return
        }
        guard !review.capabilities.isEmpty else {
            snapshot = PermissionReviewSnapshot(
                review: review,
                submissionState: .applied
            )
            return
        }
        // Managed capabilities offer no `requestedDecision` and never appear
        // in `selections`; only decidable capabilities must be fully covered.
        let decidableCount = review.capabilities
            .filter { $0.requestedDecision != nil }
            .count
        guard
            let batch = PermissionDecisionBatch(
                principal: review.principal,
                decisions: review.capabilities.compactMap { capability in
                    guard let selection = selections[capability.domain] else {
                        return nil
                    }
                    return PermissionDecisionSelection(
                        domain: capability.domain,
                        decision: selection
                    )
                }
            ),
            batch.decisions.count == decidableCount
        else {
            transientIssue = PermissionReviewIssue(
                title: "Permission review is incomplete",
                message: "Every capability needs one valid decision before confirming."
            )!
            return
        }

        isSubmitting = true
        transientIssue = nil
        await manager.submit(batch)
        let updatedSnapshot = manager.snapshot()
        if updatedSnapshot.review.principal != batch.principal {
            transientIssue = PermissionReviewIssue(
                title: "Exact build changed",
                message: "The permission review no longer matches this verified build."
            )!
        } else {
            snapshot = updatedSnapshot
        }
        isSubmitting = false
    }

    private var invalidSelections: [String] {
        review.capabilities.compactMap { capability in
            guard capability.requestedDecision != nil else {
                // Managed capabilities carry no user-selectable option. A
                // required one the host has locked out of every valid
                // decision can never launch, so it still blocks
                // confirmation; a merely optional one never does.
                let hostLockedOut = capability.decisionOptions
                    .allSatisfy { !$0.isValid }
                return capability.requirement == .required && hostLockedOut
                    ? capability.domain
                    : nil
            }
            guard
                let selected = selection(for: capability),
                capability.option(for: selected)?.isValid == true
            else {
                return capability.domain
            }
            return nil
        }
    }
}

/// A permission sheet built for the person launching a napplet, not for the
/// person who wrote it: no key material, no hashes, no NAP jargon on the
/// primary screen. Every capability gets one plain-English question and one
/// switch. Anything a developer would want -- the publisher's key, the exact
/// build hash -- is one disclosure tap away, never the default view.
public struct PermissionReviewSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State var model: PermissionReviewSheetModel
    @State private var showsTechnicalDetails = false

    @MainActor
    public init(manager: any PermissionReviewManaging) {
        _model = State(
            initialValue: PermissionReviewSheetModel(manager: manager)
        )
    }

    public var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                VStack(spacing: 0) {
                    if isUITestScrollHookEnabled {
                        scrollAnchorRow(proxy: proxy)
                    }
                    ScrollView {
                        VStack(alignment: .leading, spacing: 22) {
                            header
                            capabilityList
                            if let issue = model.issue {
                                issueView(issue)
                            }
                        }
                        .padding(20)
                    }
                }
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        model.cancel()
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                    .disabled(model.isSubmitting)
                    .accessibilityHint(
                        "Closes the review without changing any permission"
                    )
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Allow All") {
                        model.selectAllRecommended()
                        Task {
                            await model.confirm()
                            if model.isApplied {
                                dismiss()
                            }
                        }
                    }
                    .keyboardShortcut(.return, modifiers: [.command])
                    .disabled(!model.canConfirm)
                    .accessibilityIdentifier("permission-confirm")
                    .accessibilityHint(
                        "Allows every capability this napplet can be granted, at the "
                            + "broadest scope available, and saves the decision"
                    )
                }
            }
        }
        .frame(
            minWidth: 440,
            idealWidth: 500,
            minHeight: 380,
            idealHeight: 560
        )
        .interactiveDismissDisabled(model.isSubmitting)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(model.review.nappletTitle)
                .font(.title2.bold())
            Text(publisherLine)
                .font(.subheadline)
                .foregroundStyle(.secondary)
            technicalDetailsDisclosure
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(model.review.nappletTitle), \(publisherLine)")
    }

    private var publisherLine: String {
        if let name = model.review.publisherDisplayName, !name.isEmpty {
            "by \(name)"
        } else {
            "by an unverified developer"
        }
    }

    /// The publisher's key and this exact build's hash are real,
    /// verification-relevant facts -- just not ones a normal user should
    /// ever have to look at to decide whether to open a napplet. They stay
    /// available, one tap away, instead of on the primary screen.
    private var technicalDetailsDisclosure: some View {
        DisclosureGroup("Technical details", isExpanded: $showsTechnicalDetails) {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                GridRow {
                    Text("Developer key").foregroundStyle(.secondary)
                    Text(model.review.principal.manifestAuthorPublicKey)
                        .fontDesign(.monospaced)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                GridRow {
                    Text("Exact build").foregroundStyle(.secondary)
                    Text(model.review.principal.aggregateHash)
                        .fontDesign(.monospaced)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .font(.caption)
            .padding(.top, 6)
        }
        .font(.caption)
        .tint(.secondary)
        .accessibilityIdentifier("permission-technical-details")
    }

    private var capabilityList: some View {
        VStack(alignment: .leading, spacing: 10) {
            if model.review.capabilities.isEmpty {
                Label(
                    "This napplet doesn't need any special permissions.",
                    systemImage: "checkmark.shield"
                )
                .font(.callout)
                .foregroundStyle(.secondary)
            } else {
                ForEach(model.review.capabilities) { capability in
                    capabilityRow(capability)
                        .id(capability.domain)
                }
            }
        }
    }

    private func capabilityRow(
        _ capability: PermissionCapabilityReview
    ) -> some View {
        HStack(alignment: .top, spacing: 12) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: icon(for: capability.domain))
                    .font(.title3)
                    .foregroundStyle(
                        capability.sensitivity == .sensitive ? .orange : .secondary
                    )
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 6) {
                        Text(capability.title)
                            .font(.body.weight(.medium))
                        if capability.requirement == .required {
                            Text("Required")
                                .font(.caption2.weight(.semibold))
                                .foregroundStyle(.secondary)
                        }
                    }
                    Text(capability.rationale)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if capability.platformAvailability != .available {
                        availabilityNote(capability.platformAvailability)
                    }
                    if !capability.dependencies.isEmpty {
                        Text(
                            "Also needs "
                                + capability.dependencies
                                .map { humanDomainTitle($0.domain) }
                                .joined(separator: ", ")
                        )
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    }
                }
            }
            // Grouped separately from `trailingControl` below: combining this
            // description into one VoiceOver element must not swallow the
            // toggle's own accessible identity, or it silently disappears
            // from the accessibility tree entirely (confirmed against a live
            // XCUITest run -- `descendants(matching:)` could no longer find
            // it once the whole row shared one combined element).
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(capability.title). \(capability.rationale)")
            Spacer(minLength: 8)
            trailingControl(capability)
        }
        .padding(12)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 10))
    }

    @ViewBuilder
    private func trailingControl(
        _ capability: PermissionCapabilityReview
    ) -> some View {
        if capability.requestedDecision == nil {
            Label("Set by your device", systemImage: "lock.fill")
                .font(.caption)
                .foregroundStyle(.secondary)
        } else {
            Toggle(
                isOn: Binding(
                    get: { model.isGranted(capability) },
                    set: { model.setGranted($0, for: capability) }
                )
            ) {
                EmptyView()
            }
            .labelsHidden()
            .toggleStyle(.switch)
            .disabled(!model.hasAffirmativeOption(capability))
            .accessibilityIdentifier("permission-toggle-\(capability.domain)")
            .accessibilityLabel("Allow \(capability.title)")
        }
    }

    private func availabilityNote(
        _ availability: PermissionPlatformAvailability
    ) -> some View {
        Label(
            availability.detail ?? availability.title,
            systemImage: "exclamationmark.triangle.fill"
        )
        .font(.caption2)
        .foregroundStyle(.orange)
    }

    private func humanDomainTitle(_ domain: String) -> String {
        domain.replacingOccurrences(of: "-", with: " ").capitalized
    }

    private func icon(for domain: String) -> String {
        switch domain {
        case "identity": "person.crop.circle"
        case "outbox": "paperplane.fill"
        case "relay": "antenna.radiowaves.left.and.right"
        case "storage": "internaldrive"
        case "config": "gearshape.fill"
        case "resource": "photo.on.rectangle"
        case "link": "link"
        case "intent": "arrow.triangle.branch"
        case "inc": "bubble.left.and.bubble.right.fill"
        case "theme": "paintbrush.fill"
        default: "shield.fill"
        }
    }

    private func issueView(_ issue: PermissionReviewIssue) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(issue.title, systemImage: "exclamationmark.triangle")
                .font(.headline)
                .foregroundStyle(.orange)
            Text(issue.message)
            if !issue.affectedDomains.isEmpty {
                Text(
                    "Affected: "
                        + issue.affectedDomains
                        .map(humanDomainTitle)
                        .joined(separator: ", ")
                )
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(issue.title). \(issue.message)")
    }
}
