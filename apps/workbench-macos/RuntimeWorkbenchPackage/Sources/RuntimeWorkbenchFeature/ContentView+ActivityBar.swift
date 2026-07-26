import SwiftUI

extension ContentView {
    var activityBar: some View {
        let presentation = WorkbenchActivityBarPresentation(
            status: activity,
            layoutPersistenceError: layoutPersistenceError,
            capacityWarning: layout.capacityWarningMessage
        )
        return VStack(alignment: .leading, spacing: NappletMetrics.hairline) {
            HStack(spacing: 8) {
                Image(systemName: presentation.status.symbol)
                    .foregroundStyle(presentation.status.color)
                    .accessibilityHidden(true)
                Text(presentation.status.message)
                    .accessibilityIdentifier("runtime-activity")
                ForEach(presentation.layoutMessages, id: \.self) {
                    layoutMessage in
                    Divider()
                        .frame(height: 16)
                    Label(
                        layoutMessage,
                        systemImage: "externaldrive.badge.exclamationmark"
                    )
                    .foregroundStyle(NappletInk.caution)
                }
                Spacer()
                Text(presentation.policyMessage)
                    .foregroundStyle(.secondary)
            }
            if !presentation.evidenceFields.isEmpty {
                NappletEvidence(label: "Activity details") {
                    NappletFieldGrid(fields: presentation.evidenceFields)
                }
            }
        }
        .font(.caption)
        .padding(.horizontal, 16)
        .padding(.vertical, NappletMetrics.hairline)
        .frame(minHeight: 34)
        .background(.bar)
        .accessibilityElement(children: .contain)
    }
}
