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
                    accountSheetRoute = .add
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

    private var accountMenuLabel: some View {
        Image(
            systemName: activeAccount.map {
                WorkbenchAccountPresentation.symbol(for: $0)
            } ?? "person.crop.circle"
        )
        .imageScale(.large)
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
