import SwiftUI

struct PermissionCapabilityRow: View {
    let capability: PermissionCapabilityReview
    let grantBinding: Binding<Bool>
    let hasAffirmativeOption: Bool
    let isReviewLocked: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: NappletMetrics.tight) {
            Label {
                VStack(alignment: .leading, spacing: NappletMetrics.hairline) {
                    Text(phrase.sentence)
                        .font(NappletType.secondary.weight(.medium))
                        .foregroundStyle(NappletInk.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(phrase.explanation)
                        .font(NappletType.caption)
                        .foregroundStyle(NappletInk.inkSecondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } icon: {
                Image(systemName: phrase.symbol)
                    .foregroundStyle(NappletInk.inkSecondary)
            }
            .accessibilityElement(children: .combine)

            if let unavailable = unavailableMessage {
                Text(unavailable)
                    .font(NappletType.caption)
                    .foregroundStyle(NappletInk.caution)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if capability.requestedDecision == nil {
                Text(managedReason)
                    .font(NappletType.caption)
                    .foregroundStyle(NappletInk.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                grantToggle
            }
        }
    }

    private var phrase: NappletCapabilityPhrase {
        NappletVocabulary.phrase(
            forDomain: capability.domain,
            fallbackTitle: capability.title
        )
    }

    var unavailableMessage: String? {
        switch capability.platformAvailability {
        case .available:
            nil
        case .unknown:
            "This app can't tell whether that works here."
        case .unavailable:
            "Not available on this device, so it won't work."
        }
    }

    var managedReason: String {
        capability.isGranted
            ? "Allowed by a managed setting; you can't change it here."
            : "Not allowed by a managed setting; you can't change it here."
    }

    private var grantToggle: some View {
        Toggle(
            isOn: grantBinding
        ) {
            Text("Allow")
                .font(NappletType.caption)
                .foregroundStyle(NappletInk.inkSecondary)
        }
        .toggleStyle(.switch)
        .disabled(isGrantDisabled)
        .accessibilityIdentifier("permission-toggle-\(capability.domain)")
        .accessibilityLabel("Allow \(phrase.sentence)")
        .accessibilityHint(grantHint)
    }

    var isGrantDisabled: Bool {
        isReviewLocked || !hasAffirmativeOption
    }

    var grantHint: String {
        if isReviewLocked {
            return "Unavailable because this review includes managed settings"
        }
        return hasAffirmativeOption
            ? "Uses the runtime's recommended grant when switched on"
            : "The runtime did not offer an affirmative choice"
    }
}

/// The exact values behind the plain permission sentences.
struct PermissionEvidence: View {
    let review: PermissionReview
    let issue: PermissionReviewIssue?

    var body: some View {
        VStack(alignment: .leading, spacing: NappletMetrics.comfortable) {
            if let issue {
                NappletFieldGrid(fields: issueFields(issue))
            }

            NappletFieldGrid(fields: [
                NappletField(
                    "Publisher key",
                    review.principal.manifestAuthorPublicKey
                ),
                NappletField("dTag", review.principal.dTag),
                NappletField("Aggregate hash", review.principal.aggregateHash),
            ])

            ForEach(review.capabilities) { capability in
                VStack(alignment: .leading, spacing: NappletMetrics.hairline) {
                    Text(capability.domain)
                        .font(.caption.monospaced().weight(.semibold))
                    NappletFieldGrid(fields: fields(for: capability))
                }
            }
        }
    }

    private func issueFields(_ issue: PermissionReviewIssue) -> [NappletField] {
        var fields = [
            NappletField("Issue title", issue.title),
            NappletField("Issue detail", issue.message),
        ]
        if !issue.affectedDomains.isEmpty {
            fields.append(NappletField(
                "Affected domains",
                issue.affectedDomains.joined(separator: ", ")
            ))
        }
        return fields
    }

    private func fields(
        for capability: PermissionCapabilityReview
    ) -> [NappletField] {
        var fields = [
            NappletField("Title", capability.title),
            NappletField("Requirement", capability.requirement.title),
            NappletField("Sensitivity", capability.sensitivity.title),
            NappletField("Rationale", capability.rationale),
            NappletField("Availability", capability.platformAvailability.title),
            NappletField("Current decision", capability.existingDecision.title),
            NappletField("Granted", capability.isGranted ? "yes" : "no"),
        ]
        if let detail = capability.platformAvailability.detail {
            fields.append(NappletField("Availability detail", detail))
        }
        if let recommended = capability.recommendedDecision {
            fields.append(NappletField("Recommended", recommended.title))
        }
        for dependency in capability.dependencies {
            fields.append(NappletField(
                "Depends on \(dependency.domain)",
                dependency.reason
            ))
        }
        for option in capability.decisionOptions where !option.isValid {
            fields.append(NappletField(
                "\(option.decision.title) unavailable",
                option.invalidReason ?? "no reason projected"
            ))
        }
        return fields
    }
}
