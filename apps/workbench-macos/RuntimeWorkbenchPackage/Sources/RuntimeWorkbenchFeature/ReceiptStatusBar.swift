import NMPNativeRuntimeApple
import SwiftUI

/// Typed presentation of NMP's durable outcome and the independent native
/// observation lifecycle. Swift never decodes canonical receipt JSON.
struct ReceiptStatusPresentation: Equatable, Sendable {
    let title: String
    let systemImage: String
    let evidenceFields: [NappletField]

    init(
        receiptID: String,
        outcome: NativeRuntimeReceiptOutcome,
        observationLifecycle: NativeRuntimeReceiptObservationLifecycle,
        outcomeDetail: String?,
        latestStateJSON: String?
    ) {
        (title, systemImage) = outcome.presentation
        var fields = [
            NappletField("Receipt id", receiptID),
            NappletField("Durable outcome", outcome.evidenceTitle),
            NappletField(
                "Observation lifecycle",
                observationLifecycle.evidenceTitle
            ),
        ]
        if let outcomeDetail {
            fields.append(NappletField("Outcome detail", outcomeDetail))
        }
        if let latestStateJSON {
            fields.append(NappletField("Latest NMP state", latestStateJSON))
        }
        evidenceFields = fields
    }
}

/// What the runtime can truthfully say about delivery without re-deriving
/// NMP receipt semantics in Swift.
struct ReceiptStatusBar: View {
    let receipt: NativeRuntimeReceipt

    private var presentation: ReceiptStatusPresentation {
        ReceiptStatusPresentation(
            receiptID: receipt.id,
            outcome: receipt.outcome,
            observationLifecycle: receipt.observationLifecycle,
            outcomeDetail: receipt.outcomeDetail,
            latestStateJSON: receipt.latestStateJSON
        )
    }

    var body: some View {
        HStack(spacing: NappletMetrics.tight) {
            Image(systemName: presentation.systemImage)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            Text(presentation.title)
                .font(.caption)
                .accessibilityIdentifier("nap-outbox-receipt-status")
            Spacer()
            NappletEvidence(label: "Delivery details") {
                NappletFieldGrid(fields: presentation.evidenceFields)
            }
            .font(NappletType.caption)
        }
        .padding(.horizontal, NappletMetrics.comfortable)
        .padding(.vertical, NappletMetrics.hairline + 2)
        .background(.regularMaterial)
        .accessibilityElement(children: .contain)
    }
}

private extension NativeRuntimeReceiptOutcome {
    var presentation: (String, String) {
        switch self {
        case .inProgress: ("Delivery in progress", "clock")
        case .delivered: ("Delivered", "checkmark.circle")
        case .partialDelivery:
            ("Partially delivered", "exclamationmark.triangle")
        case .exhausted: ("Delivery not confirmed", "xmark.circle")
        case .ambiguous: ("Delivery outcome unknown", "questionmark.circle")
        case .refused: ("Delivery refused", "hand.raised")
        case .failed: ("Delivery failed", "exclamationmark.octagon")
        case .cancelled: ("Delivery cancelled", "xmark.circle")
        case .conflict: ("Delivery conflict", "arrow.triangle.branch")
        case .unavailable:
            ("Delivery status unavailable", "questionmark.diamond")
        }
    }

    var evidenceTitle: String {
        switch self {
        case .inProgress: "in progress"
        case .delivered: "delivered"
        case .partialDelivery: "partial delivery"
        case .exhausted: "exhausted"
        case .ambiguous: "ambiguous"
        case .refused: "refused"
        case .failed: "failed"
        case .cancelled: "cancelled"
        case .conflict: "conflict"
        case .unavailable: "unavailable"
        }
    }
}

private extension NativeRuntimeReceiptObservationLifecycle {
    var evidenceTitle: String {
        switch self {
        case .observing: "observing"
        case .notFound: "not found"
        case .closed: "closed"
        }
    }
}
