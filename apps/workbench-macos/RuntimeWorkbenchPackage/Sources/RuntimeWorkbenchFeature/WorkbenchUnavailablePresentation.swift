enum WorkbenchUnavailablePresentation: Equatable, Sendable {
    case activity(detail: String)
    case permission(detail: String)
    case relays(detail: String)

    var title: String {
        switch self {
        case .activity: "Activity unavailable"
        case .permission: "Permission review unavailable"
        case .relays: "Relays unavailable"
        }
    }

    var message: String {
        switch self {
        case .activity: "Recent activity can't be shown right now."
        case .permission: "Permission choices can't be shown right now."
        case .relays: "Relay details can't be shown right now."
        }
    }

    var symbol: String {
        switch self {
        case .activity: "waveform.path.ecg.rectangle"
        case .permission: "lock.slash"
        case .relays: "antenna.radiowaves.left.and.right.slash"
        }
    }

    var verdict: NappletTrustVerdict {
        .blocked(message)
    }

    var evidenceFields: [NappletField] {
        switch self {
        case let .activity(detail), let .permission(detail),
             let .relays(detail):
            [NappletField("Detail", detail)]
        }
    }
}
