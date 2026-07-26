import SwiftUI

extension CatalogSheet {
    var empty: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text(model.query.isEmpty ? "Nothing has arrived yet." : "Nothing matches that.")
                .font(NappletType.title)
                .foregroundStyle(NappletInk.ink)

            Text(emptyProse)
                .font(NappletType.body)
                .foregroundStyle(NappletInk.inkSecondary)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, NappletMetrics.tight)

            addressDisclosure
                .padding(.top, NappletMetrics.roomy)
        }
    }

    var emptyProse: String {
        if !model.query.isEmpty {
            return "No napplets shown here match that filter."
        }
        return "No napplets are available in this view yet. You can open one "
            + "someone sent you."
    }

    var waiting: some View {
        Text("Looking for napplets. This takes a moment the first time.")
            .font(NappletType.body)
            .foregroundStyle(NappletInk.inkSecondary)
            .fixedSize(horizontal: false, vertical: true)
    }

    var addressDisclosure: some View {
        DisclosureGroup(isExpanded: $isShowingAddress) {
            VStack(alignment: .leading, spacing: NappletMetrics.tight) {
                HStack {
                    TextField("Paste a napplet address", text: $model.manualCoordinate)
                        .textFieldStyle(.roundedBorder)
                        .font(NappletType.record)
                        .focused($focus, equals: .coordinate)
                        .onSubmit { Task { await model.reviewManualCoordinate() } }
                        .accessibilityLabel("Napplet address")

                    Button("Open") {
                        Task { await model.reviewManualCoordinate() }
                    }
                    .keyboardShortcut("i", modifiers: [.command])
                    .disabled(model.isResolvingReview || model.manualCoordinate.isEmpty)
                }
                Text("If someone sent you a napplet's address directly, paste it here.")
                    .font(NappletType.caption)
                    .foregroundStyle(NappletInk.inkSecondary)
            }
            .padding(.top, NappletMetrics.tight)
        } label: {
            Text("Have an address?")
                .font(NappletType.secondary)
                .foregroundStyle(NappletInk.inkSecondary)
        }
    }

    var footer: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(NappletInk.rule)
                .frame(height: 1)
            HStack(spacing: NappletMetrics.comfortable) {
                if let evidence = model.evidence ?? model.connectingEvidence {
                    CatalogBrowseEvidenceView(
                        evidence: evidence,
                        hasMore: model.hasMore
                    )
                }
                Spacer()
                Button("Done") {
                    model.cancelReview()
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, NappletMetrics.generous)
            .padding(.vertical, NappletMetrics.snug)
        }
        .background(NappletInk.paper)
    }

    func accessibilityLabel(for entry: CatalogEntry) -> String {
        let publisher = NappletIdentityPresentation.publisherName(
            displayName: entry.publisher.displayName,
            publicKey: entry.publisher.publicKey
        )
        return "\(entry.title), from \(publisher). \(entry.summary)"
    }
}
