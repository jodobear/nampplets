import SwiftUI

struct ActivityRefreshRefusalBanner: View {
    let refusal: RuntimeWorkbenchActivitySourceRefusal

    var body: some View {
        HStack(alignment: .top, spacing: NappletMetrics.snug) {
            Image(systemName: "clock.arrow.trianglehead.2.counterclockwise.rotate.90")
                .foregroundStyle(NappletInk.caution)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: NappletMetrics.hairline) {
                Text("Activity couldn’t refresh")
                    .font(NappletType.heading)
                Text("Showing the last accepted activity; it may be out of date.")
                    .font(NappletType.caption)
                    .foregroundStyle(NappletInk.inkSecondary)
                NappletEvidence(label: "Technical details") {
                    NappletFieldGrid(fields: evidenceFields)
                }
                .font(NappletType.caption)
            }
        }
        .padding(NappletMetrics.comfortable)
        .background(NappletInk.ground(for: .caution("")))
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    var evidenceFields: [NappletField] {
        switch refusal {
        case let .snapshotRefused(code, detail):
            [
                NappletField("Code", code),
                NappletField("Detail", detail),
            ]
        case let .subscriberCapacity(maximum):
            [
                NappletField("Refusal", refusal.localizedDescription),
                NappletField("Subscriber limit", "\(maximum)"),
            ]
        case .scopeMismatch:
            [NappletField("Refusal", refusal.localizedDescription)]
        }
    }
}
