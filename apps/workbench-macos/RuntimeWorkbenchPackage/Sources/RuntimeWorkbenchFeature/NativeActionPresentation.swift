import Foundation
import NMPNativeRuntimeApple

/// A presentation-only projection of one Rust-typed NAP-INC action.
///
/// Swift never interprets `payloadJSON`. Rust owns payload validation and
/// semantics; the entire opaque payload remains available as evidence.
struct NativeActionNotice: Identifiable, Equatable, Sendable {
    let id: UUID
    let kind: NativeWorkbenchActionKind
    let title: String
    /// Plain language. Never contains an identifier.
    let summary: String
    /// The identifiers, for the technical tier only.
    let evidence: [NappletField]

    init(
        id: UUID = UUID(),
        kind: NativeWorkbenchActionKind,
        title: String,
        summary: String,
        evidence: [NappletField] = []
    ) {
        self.id = id
        self.kind = kind
        self.title = title
        self.summary = summary
        self.evidence = evidence
    }

    static func presentation(
        _ action: NativeWorkbenchAction
    ) -> NativeActionNotice {
        let copy: (title: String, summary: String)
        switch action.kind {
        case .noteOpen:
            copy = (
                "Open a post",
                "This napplet wants to show you a post."
            )
        case .profileOpen:
            copy = (
                "Open a profile",
                "This napplet wants to show you someone's profile."
            )
        case .composeOpen:
            copy = (
                "Write something",
                "This napplet wants you to write a post. "
                    + "Napplets can't do that here yet.",
            )
        }

        return NativeActionNotice(
            kind: action.kind,
            title: copy.title,
            summary: copy.summary,
            evidence: [
                NappletField("Manifest author", action.manifestAuthor),
                NappletField("dTag", action.dTag),
                NappletField("Aggregate hash", action.aggregateHash),
                NappletField("Action kind", action.kind.rawValue),
                NappletField("Source session", "\(action.sessionID)"),
                NappletField("Source window", "\(action.sourceWindowID)"),
                NappletField("Payload JSON", action.payloadJSON),
            ]
        )
    }
}
