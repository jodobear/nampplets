import SwiftUI

enum WorkbenchActivityStatus: Equatable, Sendable {
    case preparing
    case readyToAdd
    case ready
    case opening
    case reopening
    case restoring(count: Int)
    case waitingForPermission
    case loading(title: String)
    case running(title: String)
    case closed(title: String)
    case nativeAction(title: String, nappletTitle: String)
    case refused(detail: String)
    case failed(detail: String)
    case crashed(title: String)

    var message: String {
        switch self {
        case .preparing: "Getting things ready"
        case .readyToAdd: "Ready · add a napplet"
        case .ready: "Ready"
        case .opening: "Opening…"
        case .reopening: "Reopening…"
        case let .restoring(count):
            "Reopening \(count) saved napplet\(count == 1 ? "" : "s")"
        case .waitingForPermission: "Waiting for your permission"
        case let .loading(title): "Opening \(title)…"
        case let .running(title): "\(title) is running"
        case let .closed(title): "Closed \(title). It's still installed."
        case let .nativeAction(title, nappletTitle):
            "\(title) from \(nappletTitle)"
        case .refused: "That action was refused."
        case .failed: "That action couldn't be completed."
        case let .crashed(title): "\(title) stopped unexpectedly"
        }
    }

    var technicalDetail: String? {
        switch self {
        case let .refused(detail), let .failed(detail): detail
        default: nil
        }
    }

    var symbol: String {
        switch self {
        case .refused, .failed, .crashed: "exclamationmark.triangle"
        default: "circle"
        }
    }

    var color: Color {
        switch self {
        case .refused, .failed, .crashed: NappletInk.refusal
        default: NappletInk.inkSecondary
        }
    }
}

struct WorkbenchActivityBarPresentation: Equatable, Sendable {
    let status: WorkbenchActivityStatus
    let layoutMessages: [String]
    let evidenceFields: [NappletField]
    let policyMessage =
        "Napplet access follows your choices and managed settings."

    init(
        status: WorkbenchActivityStatus,
        layoutPersistenceError: String?,
        capacityWarning: String?
    ) {
        self.status = status
        var fields: [NappletField] = []
        var messages: [String] = []
        if let detail = status.technicalDetail {
            fields.append(NappletField("Activity detail", detail))
        }
        if let layoutPersistenceError {
            messages.append("Layout changes couldn't be saved.")
            fields.append(
                NappletField(
                    "Layout persistence detail",
                    layoutPersistenceError
                )
            )
        }
        if let capacityWarning {
            messages.append("Some saved windows couldn't be restored.")
            fields.append(
                NappletField("Layout capacity detail", capacityWarning)
            )
        }
        layoutMessages = messages
        evidenceFields = fields
    }
}
