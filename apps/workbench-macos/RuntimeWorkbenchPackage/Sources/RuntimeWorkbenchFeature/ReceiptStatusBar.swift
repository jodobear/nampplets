import NMPNativeRuntimeApple
import SwiftUI

/// Neutral presentation of a receipt observation.
///
/// Publication outcome and lifecycle both belong to NMP. The shell preserves
/// Rust's typed status as evidence without deriving its own success styling or
/// treating the observation lifecycle string as a delivery verdict.
struct ReceiptStatusPresentation: Equatable, Sendable {
    let title = "Delivery details available"
    let systemImage = "doc.text.magnifyingglass"
    let evidenceFields: [NappletField]

    init(
        receiptID: String,
        status: NativeRuntimeReceiptStatus,
        delivery: String,
        latestStateJSON: String?
    ) {
        var fields = [
            NappletField("Receipt id", receiptID),
            NappletField("Runtime status", status.evidenceTitle),
            NappletField("Observation lifecycle", delivery),
        ]
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
            status: receipt.status,
            delivery: receipt.delivery,
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

private extension NativeRuntimeReceiptStatus {
    var evidenceTitle: String {
        switch self {
        case .pending: "pending"
        case .delivered: "delivered"
        }
    }
}
