import SwiftUI

struct ActivityFactRow: View {
    let fact: ActivityFact
    let detailFields: [ActivityDetailField]

    var body: some View {
        HStack(alignment: .top, spacing: NappletMetrics.snug) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
                .frame(width: 22)
                .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: NappletMetrics.hairline + 1) {
                HStack(alignment: .firstTextBaseline) {
                    Text(fact.title)
                        .font(NappletType.heading)
                        .foregroundStyle(NappletInk.ink)
                    Spacer()
                    Text(fact.kind.title)
                        .font(NappletType.caption)
                        .foregroundStyle(NappletInk.inkSecondary)
                }

                Text(fact.summary)
                    .font(NappletType.secondary)
                    .foregroundStyle(NappletInk.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)

                if fact.evidenceSummary != nil || !detailFields.isEmpty {
                    NappletEvidence(label: "Details") {
                        NappletFieldGrid(fields: evidenceFields)
                    }
                    .font(NappletType.caption)
                }
            }
        }
        .padding(.vertical, NappletMetrics.hairline + 1)
        .accessibilityElement(children: .contain)
        .accessibilityLabel(
            "\(fact.severity.title), \(fact.kind.title), "
                + "\(fact.title). \(fact.summary)"
        )
    }

    private var evidenceFields: [NappletField] {
        var fields: [NappletField] = []
        if let evidence = fact.evidenceSummary {
            fields.append(NappletField("Evidence", evidence))
        }
        fields.append(contentsOf: detailFields.map { field in
            NappletField(field.key, field.displayValue)
        })
        return fields
    }

    private var symbol: String {
        switch fact.kind {
        case .providerCall: "arrow.left.arrow.right"
        case .providerRefusal: "hand.raised"
        case .activeSession: "play.rectangle"
        case .activeBinding: "link"
        case .activeResource: "shippingbox"
        case .pendingReceipt: "clock"
        case .crash: "bolt.trianglebadge.exclamationmark"
        case .recovery: "cross.case"
        }
    }

    /// Colour reinforces the printed severity; it never carries meaning alone.
    private var tint: Color {
        switch fact.severity {
        case .debug, .information: NappletInk.inkSecondary
        case .warning: NappletInk.caution
        case .error: NappletInk.refusal
        }
    }
}
