enum WorkbenchAccountPresentation {
    static func name(
        for account: WorkbenchStoredAccount,
        in accounts: [WorkbenchStoredAccount]
    ) -> String {
        let peers = accounts.filter {
            $0.connectionKind == account.connectionKind
        }
        let base = switch account.connectionKind {
        case .localSigner:
            "Signing Account"
        case .remoteSigner:
            "Connected Account"
        case .readOnly:
            "Browsing Profile"
        }
        guard
            peers.count > 1,
            let index = peers.firstIndex(where: {
                $0.handle == account.handle
            })
        else {
            return base
        }
        return "\(base) \(index + 1)"
    }

    static func detail(for account: WorkbenchStoredAccount) -> String {
        account.connectionKind.title
    }

    static func symbol(for account: WorkbenchStoredAccount) -> String {
        switch account.connectionKind {
        case .localSigner:
            "person.crop.circle"
        case .remoteSigner:
            "person.crop.circle.badge.checkmark"
        case .readOnly:
            "eye.circle"
        }
    }
}
