import SwiftUI

extension ContentView {
    var accountMenu: some View {
        Menu {
            if !accountSnapshot.accounts.isEmpty {
                accountChoices

                if accountSnapshot.activeAccount != nil {
                    Button(
                        "Sign Out",
                        systemImage: "rectangle.portrait.and.arrow.right"
                    ) {
                        Task {
                            await accountManager.logout()
                            accountSnapshot = accountManager.snapshot()
                        }
                    }
                }
            }

            Section {
                Button("Add Account…", systemImage: "key") {
                    accountSheetRoute = .secret
                }
                Button("Browse Without Signing…", systemImage: "eye") {
                    accountSheetRoute = .readOnly
                }
            }

            if !accountSnapshot.accounts.isEmpty {
                Button("Manage Accounts…", systemImage: "person.2") {
                    accountSheetRoute = .manage
                }
            }
        } label: {
            accountMenuLabel
        }
        .menuStyle(.borderlessButton)
        .help(activeAccount == nil ? "Add or Choose an Account" : "Account")
        .accessibilityLabel("Account")
        .accessibilityValue(activeAccountLabel)
        .accessibilityIdentifier("account-switcher")
    }

    private var accountChoices: some View {
        Section("Accounts") {
            ForEach(accountSnapshot.accounts) { account in
                Button {
                    Task {
                        await accountManager.activate(handle: account.handle)
                        accountSnapshot = accountManager.snapshot()
                    }
                } label: {
                    if accountSnapshot.activeHandle == account.handle {
                        Label(
                            accountDisplayName(account),
                            systemImage: "checkmark"
                        )
                    } else {
                        Label(
                            accountDisplayName(account),
                            systemImage: WorkbenchAccountPresentation.symbol(
                                for: account
                            )
                        )
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var accountMenuLabel: some View {
        if let activeAccount {
            HStack(spacing: 6) {
                Image(
                    systemName: WorkbenchAccountPresentation.symbol(
                        for: activeAccount
                    )
                )
                Text(accountDisplayName(activeAccount))
                    .lineLimit(1)
            }
        } else {
            Image(systemName: "person.crop.circle")
        }
    }

    private var activeAccountLabel: String {
        activeAccount.map(accountDisplayName) ?? "No account selected"
    }

    private var activeAccount: WorkbenchStoredAccount? {
        guard let activeHandle = accountSnapshot.activeHandle else {
            return nil
        }
        return accountSnapshot.accounts.first {
            $0.handle == activeHandle
        }
    }

    private func accountDisplayName(
        _ account: WorkbenchStoredAccount
    ) -> String {
        WorkbenchAccountPresentation.name(
            for: account,
            in: accountSnapshot.accounts
        )
    }
}
