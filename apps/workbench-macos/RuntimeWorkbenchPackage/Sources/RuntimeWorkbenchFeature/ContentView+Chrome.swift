import NMPNativeRuntimeApple
import SwiftUI

extension ContentView {
    @ViewBuilder
    var platformBody: some View {
        #if os(iOS)
        if layout.mode == .fullWindow {
            WorkbenchFullWindowView(
                layout: $layout,
                rootID: fullWindowRootID,
                path: $fullWindowPath,
                onExit: exitFullWindow,
                windowContent: windowContent,
                topBars: { topStatusBars }
            )
        } else {
            NavigationStack {
                canvasBody
                    .navigationTitle("Napplets")
                    .navigationBarTitleDisplayMode(.inline)
                    .toolbar {
                        ToolbarItem(placement: .topBarLeading) {
                            accountMenu
                        }
                        ToolbarItemGroup(placement: .topBarTrailing) {
                            Button {
                                isCatalogSheetPresented = true
                            } label: {
                                Label("Add Napplet", systemImage: "plus")
                            }
                            .accessibilityIdentifier("add-napplet")
                            .accessibilityHint("Opens the network napplet catalog")

                            workspaceActionsMenu

                            Button {
                                withAnimation(.easeInOut(duration: 0.18)) {
                                    isInspectorPresented.toggle()
                                }
                            } label: {
                                Label(
                                    isInspectorPresented ? "Hide Inspector" : "Show Inspector",
                                    systemImage: "sidebar.right"
                                )
                            }
                            .accessibilityIdentifier("toggle-napplet-inspector")

                            layoutMenu
                        }
                    }
            }
        }
        #else
        VStack(spacing: 0) {
            workspaceControlStrip
            canvasBody
        }
        #endif
    }

    @ViewBuilder
    var topStatusBars: some View {
        if let pendingWrite = pendingWrites.writes.first {
            PendingWriteApprovalBar(write: pendingWrite) { approve in
                pendingWrites.decide(
                    pendingWrite,
                    approve: approve,
                    profile: profile
                )
            }
        }
        if let receipt = receipts.receipts.last {
            ReceiptStatusBar(receipt: receipt)
        }
    }

    private var workspaceControlStrip: some View {
        HStack(spacing: 10) {
            accountMenu

            Text("Napplets")
                .font(.title3.weight(.semibold))
            Spacer()

            Button {
                isCatalogSheetPresented = true
            } label: {
                Label("Add Napplet", systemImage: "plus")
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut("n", modifiers: [.command])
            .accessibilityIdentifier("add-napplet")
            .accessibilityHint("Opens the network napplet catalog")

            workspaceActionsMenu

            Button {
                withAnimation(.easeInOut(duration: 0.18)) {
                    isInspectorPresented.toggle()
                }
            } label: {
                Label(
                    isInspectorPresented ? "Hide Inspector" : "Show Inspector",
                    systemImage: "sidebar.right"
                )
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .keyboardShortcut("i", modifiers: [.command, .option])
            .accessibilityIdentifier("toggle-napplet-inspector")

            layoutMenu
        }
        .padding(.horizontal, 14)
        .frame(height: 50)
        .background(.bar)
    }

    private var accountMenu: some View {
        Menu {
            Section("Switch account") {
                if accountSnapshot.accounts.isEmpty {
                    Text("No stored accounts")
                } else {
                    ForEach(accountSnapshot.accounts) { account in
                        Button {
                            Task {
                                await accountManager.activate(
                                    handle: account.handle
                                )
                                accountSnapshot = accountManager.snapshot()
                            }
                        } label: {
                            if accountSnapshot.activeHandle == account.handle {
                                Label(
                                    "\(accountDisplayName(account)) · "
                                        + account.connectionKind.title,
                                    systemImage: "checkmark"
                                )
                            } else {
                                Label(
                                    "\(accountDisplayName(account)) · "
                                        + account.connectionKind.title,
                                    systemImage: accountMenuSymbol(account)
                                )
                            }
                        }
                    }
                }

                if accountSnapshot.activeAccount != nil {
                    Button("Sign Out", systemImage: "rectangle.portrait.and.arrow.right") {
                        Task {
                            await accountManager.logout()
                            accountSnapshot = accountManager.snapshot()
                        }
                    }
                }
            }

            Section("Add account") {
                Button("Signer-backed Account…", systemImage: "key") {
                    isAccountSheetPresented = true
                }
                Button(
                    "Read-only Identity…",
                    systemImage: "person.text.rectangle"
                ) {
                    isAccountSheetPresented = true
                }
            }

            Button("Manage Accounts…", systemImage: "person.2") {
                isAccountSheetPresented = true
            }
        } label: {
            Label(
                activeAccountLabel,
                systemImage: accountSnapshot.activeAccount == nil
                    ? "person.crop.circle.badge.xmark"
                    : "person.crop.circle.badge.checkmark"
            )
        }
        .menuStyle(.borderlessButton)
        .accessibilityLabel("Account switcher")
        .accessibilityValue(activeAccountLabel)
        .accessibilityIdentifier("account-switcher")
    }

    private var activeAccountLabel: String {
        guard
            let activeHandle = accountSnapshot.activeHandle,
            let account = accountSnapshot.accounts.first(where: {
                $0.handle == activeHandle
            })
        else {
            return "Signed Out"
        }
        return accountDisplayName(account)
    }

    private func accountDisplayName(
        _ account: WorkbenchStoredAccount
    ) -> String {
        let projectedIdentity = account.npub.isEmpty
            ? account.publicKeyHex
            : account.npub
        guard projectedIdentity.count > 16 else {
            return projectedIdentity.isEmpty
                ? account.connectionKind.title
                : projectedIdentity
        }
        return "\(projectedIdentity.prefix(8))…\(projectedIdentity.suffix(6))"
    }

    private func accountMenuSymbol(
        _ account: WorkbenchStoredAccount
    ) -> String {
        switch account.connectionKind {
        case .localSigner:
            "key"
        case .remoteSigner:
            "network"
        case .readOnly:
            "eye"
        }
    }

    @MainActor
    func setLayoutMode(_ mode: WorkbenchLayoutMode) {
        mutateLayout { $0.setMode(mode) }
        if mode == .fullWindow {
            fullWindowRootID = layout.selectedWindow?.id
            fullWindowPath = []
        } else {
            fullWindowRootID = nil
            fullWindowPath = []
        }
    }

    @MainActor
    private func exitFullWindow() {
        setLayoutMode(.freeform)
    }
}
