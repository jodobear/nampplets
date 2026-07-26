import SwiftUI

enum CatalogInstallPlainCopy {
    static let optionalHeading = "The napplet lists these as optional"
    static let reassurance =
        "Adding doesn't grant access or open the napplet. "
        + "Access is reviewed separately."
}

/// What a person sees before adding a napplet.
///
/// Adding acquires verified bytes and grants nothing -- the runtime asks for
/// capability at first run, not here. So this surface is deliberately light:
/// it says what the napplet is, who made it, and what it will ask for later,
/// and it puts every hash, coordinate and provenance record behind one
/// deliberate move. See `docs/adr/0008-verdicts-on-the-path.md`.
struct CatalogInstallReviewSheet: View {
    let review: CatalogInstallReview
    let isInstalling: Bool
    let issuePresentation: CatalogIssueNotice.Presentation?
    let onCancel: () -> Void
    let onConfirm: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 0) {
                    Text(review.title)
                        .font(NappletType.display)
                        .nappletDisplayFace()
                        .foregroundStyle(NappletInk.ink)
                        .fixedSize(horizontal: false, vertical: true)

                    Text(byline)
                        .font(NappletType.lede)
                        .foregroundStyle(NappletInk.inkSecondary)
                        .padding(.top, NappletMetrics.tight)

                    NappletNotice(verdict: verdict)
                        .padding(.top, NappletMetrics.roomy)

                    if let issuePresentation {
                        CatalogIssueNotice(presentation: issuePresentation)
                            .padding(.top, NappletMetrics.snug)
                    }

                    capabilities
                        .padding(.top, NappletMetrics.spacious)

                    reassurance
                        .padding(.top, NappletMetrics.roomy)

                    NappletEvidence {
                        CatalogInstallEvidence(review: review)
                    }
                    .font(NappletType.caption)
                    .padding(.top, NappletMetrics.roomy)
                }
                .frame(maxWidth: NappletMetrics.measure, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, NappletMetrics.generous)
                .padding(.top, NappletMetrics.generous)
                .padding(.bottom, NappletMetrics.spacious)
            }

            actions
        }
        .background(NappletInk.paperRaised)
        #if os(macOS)
        .frame(minWidth: 560, idealWidth: 620, minHeight: 480, idealHeight: 660)
        #endif
        .interactiveDismissDisabled(isInstalling)
    }

    /// The accent appears here and nowhere else on this screen.
    private var actions: some View {
        VStack(spacing: 0) {
            Rectangle()
                .fill(NappletInk.rule)
                .frame(height: 1)
            AdaptiveActionPair {
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
            } trailing: {
                Button(
                    isInstalling ? "Adding…" : "Add Napplet",
                    action: onConfirm
                )
                .buttonStyle(.borderedProminent)
                .tint(NappletInk.accent)
                .keyboardShortcut(.defaultAction)
                .disabled(!review.canInstall || isInstalling)
                .accessibilityIdentifier("catalog-install-exact-build")
                .accessibilityHint(
                    "Adds this napplet. It cannot do anything until you open it."
                )
            }
            .padding(.horizontal, NappletMetrics.generous)
            .padding(.vertical, NappletMetrics.comfortable)
        }
    }

    private var byline: String {
        NappletIdentityPresentation.publisherIsUnnamed(
            displayName: review.publisher.displayName,
            publicKey: review.publisher.publicKey
        )
            ? "From a publisher who hasn't given a name"
            : "by " + NappletIdentityPresentation.publisherName(
                displayName: review.publisher.displayName,
                publicKey: review.publisher.publicKey
            )
    }

    /// The one thing a person is actually deciding about.
    /// The napplet's claim about itself, so it is set as a card. The group
    /// headings live on the page outside it: heading-card-heading-card nesting
    /// is the grouped-Form look this redesign exists to escape.
    @ViewBuilder
    private var capabilities: some View {
        if review.requiredDomains.isEmpty, review.optionalDomains.isEmpty {
            VStack(alignment: .leading, spacing: NappletMetrics.snug) {
                Text("What it will ask for")
                    .font(NappletType.heading)
                    .foregroundStyle(NappletInk.ink)
                Text("Nothing. This napplet doesn't ask for access to anything.")
                    .font(NappletType.secondary)
                    .foregroundStyle(NappletInk.inkSecondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else {
            VStack(alignment: .leading, spacing: NappletMetrics.snug) {
                Text("What it will ask for")
                    .font(NappletType.heading)
                    .foregroundStyle(NappletInk.ink)

                VStack(alignment: .leading, spacing: NappletMetrics.comfortable) {
                    if !review.requiredDomains.isEmpty {
                        capabilityList(review.requiredDomains)
                    }
                    if !review.optionalDomains.isEmpty {
                        VStack(alignment: .leading, spacing: NappletMetrics.snug) {
                            Text(CatalogInstallPlainCopy.optionalHeading)
                                .font(NappletType.caption)
                                .foregroundStyle(NappletInk.inkSecondary)
                            capabilityList(review.optionalDomains)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(NappletMetrics.comfortable)
                .background(
                    NappletInk.fillQuiet,
                    in: RoundedRectangle(
                        cornerRadius: NappletMetrics.cardCorner,
                        style: .continuous
                    )
                )
            }
        }
    }

    private func capabilityList(_ domains: [String]) -> some View {
        VStack(alignment: .leading, spacing: NappletMetrics.snug) {
            ForEach(domains, id: \.self) { domain in
                let phrase = NappletVocabulary.phrase(forDomain: domain)
                Label {
                    Text(phrase.sentence)
                        .foregroundStyle(NappletInk.ink)
                        .fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: phrase.symbol)
                        .foregroundStyle(NappletInk.inkSecondary)
                }
                .font(NappletType.secondary)
                .accessibilityElement(children: .combine)
            }
        }
    }

    private var reassurance: some View {
        Text(
            CatalogInstallPlainCopy.reassurance
        )
        .font(NappletType.secondary)
        .foregroundStyle(NappletInk.inkSecondary)
        .fixedSize(horizontal: false, vertical: true)
    }

    /// Verdicts only, and only when there is something to say. Rust owns
    /// whether an install may proceed (`canInstall`) and which warnings are
    /// blocking; this reads those decisions rather than re-deriving them.
    var verdict: NappletTrustVerdict {
        if review.warnings.contains(where: { $0.severity == .blocking }) {
            return .blocked("This napplet can't be added right now.")
        }
        if currentPlatformIsIncompatible {
            return .blocked("This napplet doesn't run on this device.")
        }
        if !review.canInstall {
            return .blocked("This napplet can't be added right now.")
        }
        if review.warnings.contains(where: { $0.severity == .caution }) {
            return .caution("There's something to review before adding this napplet.")
        }
        return relationshipVerdict
    }

    private var relationshipVerdict: NappletTrustVerdict {
        switch review.updateRelationship {
        case .sameBuild:
            .caution("You already have this napplet.")
        case .rollback:
            .caution("This is an older version than the one you already have.")
        case .differentBuild:
            .caution(
                "You already have a different version of this napplet. "
                    + "Adding this one replaces it."
            )
        case .update, .firstInstall, .unknown:
            .settled
        }
    }

    private var currentPlatformIsIncompatible: Bool {
        #if os(macOS)
        let current = "macos"
        #else
        let current = "ios"
        #endif
        return review.platformCompatibility.contains(where: { row in
                row.platform
                    .lowercased()
                    .replacingOccurrences(of: " ", with: "") == current
                    && row.status == .incompatible
            })
    }
}
