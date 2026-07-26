import SwiftUI

struct WorkbenchAccountManagementSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: WorkbenchAccountSheetModel
    @State private var pendingRemoval: WorkbenchStoredAccount?

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Accounts")
                    .font(.title2.weight(.semibold))
                Text("Choose the account napplets use on this Mac.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(24)

            Divider()

            if model.snapshot.accounts.isEmpty {
                ContentUnavailableView(
                    "No accounts yet",
                    systemImage: "person.crop.circle",
                    description: Text(
                        "Add an account from the account menu in the toolbar."
                    )
                )
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List {
                    ForEach(
                        Array(model.snapshot.accounts.enumerated()),
                        id: \.element.id
                    ) { index, account in
                        accountRow(account, index: index)
                    }
                }
                .listStyle(.inset)
            }

            if let errorMessage = model.errorMessage {
                Label(errorMessage, systemImage: "exclamationmark.circle")
                    .font(.callout)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 24)
                    .padding(.vertical, 12)
            }

            Divider()

            HStack {
                Spacer()
                Button("Close") {
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
            }
            .padding(16)
        }
        #if os(macOS)
        .frame(width: 500, height: 390)
        #endif
        .confirmationDialog(
            "Remove this account?",
            isPresented: Binding(
                get: { pendingRemoval != nil },
                set: { if !$0 { pendingRemoval = nil } }
            ),
            presenting: pendingRemoval
        ) { account in
            Button("Remove Account", role: .destructive) {
                pendingRemoval = nil
                Task {
                    await model.remove(account.handle)
                }
            }
            Button("Cancel", role: .cancel) {
                pendingRemoval = nil
            }
        } message: { _ in
            Text("You’ll need its account key to add it again.")
        }
    }

    private func accountRow(
        _ account: WorkbenchStoredAccount,
        index: Int
    ) -> some View {
        HStack(spacing: 12) {
            Image(
                systemName: WorkbenchAccountPresentation.symbol(for: account)
            )
            .font(.title2)
            .foregroundStyle(.secondary)
            .accessibilityHidden(true)

            VStack(alignment: .leading, spacing: 3) {
                Text(
                    WorkbenchAccountPresentation.name(
                        for: account,
                        in: model.snapshot.accounts
                    )
                )
                .lineLimit(1)
                Text(WorkbenchAccountPresentation.detail(for: account))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()

            if account.handle == model.snapshot.activeHandle {
                Text("Current")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
            }

            Menu {
                if account.handle == model.snapshot.activeHandle {
                    Button(
                        "Sign Out",
                        systemImage: "rectangle.portrait.and.arrow.right"
                    ) {
                        Task {
                            await model.logout()
                        }
                    }
                } else {
                    Button("Use This Account", systemImage: "checkmark") {
                        Task {
                            await model.activate(account.handle)
                        }
                    }
                }

                Divider()

                Button(
                    "Remove Account",
                    systemImage: "trash",
                    role: .destructive
                ) {
                    pendingRemoval = account
                }
            } label: {
                Label("Account actions", systemImage: "ellipsis.circle")
            }
            .labelStyle(.iconOnly)
            .menuStyle(.borderlessButton)
            .disabled(model.isWorking)
            .help("Account Actions")
            .accessibilityIdentifier("account-actions-\(index)")
            .accessibilityLabel("Account \(index + 1) actions")
        }
        .padding(.vertical, 6)
    }
}
