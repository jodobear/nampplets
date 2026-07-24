import Foundation
import NMPNativeRuntimeApple

/// A bounded, native-only projection of one NAP-INC action.
///
/// The napplet payload is not treated as navigation authority. It is decoded
/// only for the small schemas accepted by the pinned provider, and the
/// Workbench still scopes presentation to the exact installed build that
/// emitted the action.
struct NativeActionNotice: Identifiable, Equatable, Sendable {
    let id: UUID
    let kind: NativeWorkbenchActionKind
    let title: String
    let target: String
    let detail: String

    init(
        id: UUID = UUID(),
        kind: NativeWorkbenchActionKind,
        title: String,
        target: String,
        detail: String
    ) {
        self.id = id
        self.kind = kind
        self.title = title
        self.target = target
        self.detail = detail
    }

    static func decode(
        _ action: NativeWorkbenchAction
    ) -> NativeActionNotice? {
        guard let object = try? JSONSerialization.jsonObject(
            with: Data(action.payloadJSON.utf8),
            options: [.fragmentsAllowed]
        ) as? [String: Any]
        else {
            return nil
        }

        switch action.kind {
        case .noteOpen:
            guard
                let target = object["target"] as? [String: Any],
                target["type"] as? String == "event",
                let eventID = boundedHex(target["id"], length: 64)
            else {
                return nil
            }
            let kind = boundedInteger(target["kind"])
            let author = boundedHex(target["pubkey"], length: 64)
            let targetText = kind.map { "event \(eventID) · kind \($0)" }
                ?? "event \(eventID)"
            let detail = author.map { "Author \($0)" }
                ?? "The napplet requested a note target."
            return NativeActionNotice(
                kind: action.kind,
                title: "Note requested",
                target: targetText,
                detail: detail
            )
        case .profileOpen:
            guard let pubkey = boundedHex(object["pubkey"], length: 64) else {
                return nil
            }
            return NativeActionNotice(
                kind: action.kind,
                title: "Profile requested",
                target: pubkey,
                detail: "The napplet requested a profile target."
            )
        case .composeOpen:
            guard let replyTo = object["replyTo"] as? [String: Any] else {
                return NativeActionNotice(
                    kind: action.kind,
                    title: "Compose requested",
                    target: "No reply target",
                    detail: "The Workbench does not provide a composer."
                )
            }
            let target = boundedHex(replyTo["id"], length: 64)
                .map { "reply to event \($0)" }
                ?? "reply target unavailable"
            return NativeActionNotice(
                kind: action.kind,
                title: "Compose requested",
                target: target,
                detail: "The Workbench does not provide a composer."
            )
        }
    }

    private static func boundedHex(
        _ value: Any?,
        length: Int
    ) -> String? {
        guard let value = value as? String,
              value.utf8.count == length,
              value.unicodeScalars.allSatisfy(
                  CharacterSet(charactersIn: "0123456789abcdefABCDEF").contains
              )
        else {
            return nil
        }
        return value
    }

    private static func boundedInteger(_ value: Any?) -> Int? {
        guard let number = value as? NSNumber,
              number.doubleValue.rounded() == number.doubleValue,
              number.intValue >= 0,
              number.intValue <= 65_535
        else {
            return nil
        }
        return number.intValue
    }
}
