import SwiftUI

struct WorkbenchRelayLaneEditor: View {
    let title: String
    let detail: String
    let systemImage: String
    let identifierPrefix: String
    @Binding var relays: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.body.weight(.medium))
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } icon: {
                Image(systemName: systemImage)
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 24)
                    .accessibilityHidden(true)
            }

            Divider()

            ForEach(relays.indices, id: \.self) { index in
                HStack(spacing: 10) {
                    TextField(
                        "Relay address",
                        text: Binding(
                            get: { relays[index] },
                            set: { relays[index] = $0 }
                        ),
                        prompt: Text("wss://relay.example")
                    )
                    .labelsHidden()
                    .textFieldStyle(.plain)
                    .padding(.horizontal, 9)
                    .frame(height: 30)
                    .background(
                        Color.primary.opacity(0.045),
                        in: .rect(cornerRadius: 7)
                    )
                    .autocorrectionDisabled()
                    .accessibilityLabel("\(title) address \(index + 1)")
                    .accessibilityIdentifier(
                        "\(identifierPrefix)-relay-\(index)"
                    )

                    Button {
                        relays.remove(at: index)
                    } label: {
                        Image(systemName: "minus.circle.fill")
                            .symbolRenderingMode(.hierarchical)
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Remove \(title) address")
                }
            }

            Button {
                relays.append("")
            } label: {
                Label("Add relay", systemImage: "plus")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.tint)
            .disabled(
                relays.count
                    >= WorkbenchProfilePreferences.maximumRelaysPerGroup
            )
            .accessibilityIdentifier("\(identifierPrefix)-relay-add")
        }
        .padding(.vertical, 6)
    }
}
