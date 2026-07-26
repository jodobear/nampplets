import SwiftUI

/// Everything the runtime verified, verbatim, for the person who asked.
struct CatalogInstallEvidence: View {
    let review: CatalogInstallReview

    var body: some View {
        VStack(alignment: .leading, spacing: NappletMetrics.comfortable) {
            NappletFieldGrid(fields: identityFields)

            if !review.requiredDomains.isEmpty || !review.optionalDomains.isEmpty {
                evidenceSection("Capability domains") {
                    NappletFieldGrid(fields: [
                        NappletField(
                            "Required",
                            review.requiredDomains.isEmpty
                                ? "none"
                                : review.requiredDomains.joined(separator: ", ")
                        ),
                        NappletField(
                            "Optional",
                            review.optionalDomains.isEmpty
                                ? "none"
                                : review.optionalDomains.joined(separator: ", ")
                        ),
                    ])
                }
            }

            if !review.sources.isEmpty {
                evidenceSection("Sources and provenance") {
                    VStack(alignment: .leading, spacing: NappletMetrics.snug) {
                        ForEach(review.sources) { source in
                            VStack(alignment: .leading, spacing: 2) {
                                Text(source.kind.rawValue)
                                    .font(.caption.weight(.semibold))
                                Text(source.source)
                                    .font(.caption.monospaced())
                                Text(source.evidence)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            .accessibilityElement(children: .combine)
                        }
                    }
                }
            }

            if !review.platformCompatibility.isEmpty {
                evidenceSection("Platform compatibility") {
                    NappletFieldGrid(
                        fields: review.platformCompatibility.map { row in
                            NappletField(
                                row.platform,
                                "\(statusWord(row.status)) — \(row.detail)"
                            )
                        }
                    )
                }
            }

            if !review.warnings.isEmpty {
                evidenceSection("Warnings") {
                    NappletFieldGrid(
                        fields: review.warnings.map { warning in
                            NappletField(
                                severityWord(warning.severity),
                                warning.message
                            )
                        }
                    )
                }
            }
        }
    }

    private var identityFields: [NappletField] {
        var fields = [
            NappletField("Publisher key", review.publisher.publicKey),
            NappletField("Coordinate", review.coordinate),
            NappletField("Aggregate hash", review.exactAggregateHash),
            NappletField("Relationship", review.updateRelationship.title),
        ]
        if let installedHash = review.updateRelationship.installedHash {
            fields.append(NappletField("Installed hash", installedHash))
        }
        if let detail = review.updateRelationship.detail {
            fields.append(NappletField("Relationship detail", detail))
        }
        return fields
    }

    private func evidenceSection(
        _ title: String,
        @ViewBuilder content: () -> some View
    ) -> some View {
        VStack(alignment: .leading, spacing: NappletMetrics.tight) {
            Text(title)
                .font(.caption.weight(.semibold))
                .foregroundStyle(.secondary)
            content()
        }
    }

    private func statusWord(_ status: CatalogPlatformStatus) -> String {
        switch status {
        case .compatible: "compatible"
        case .incompatible: "incompatible"
        case .unavailable: "unavailable"
        }
    }

    private func severityWord(_ severity: CatalogWarningSeverity) -> String {
        switch severity {
        case .information: "info"
        case .caution: "caution"
        case .blocking: "blocking"
        }
    }
}
