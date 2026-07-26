import SwiftUI

enum WorkbenchAccountSheetRoute: String, Identifiable {
    case add
    case manage

    var id: String { rawValue }
}

struct WorkbenchAccountSheet: View {
    @State private var model: WorkbenchAccountSheetModel
    let route: WorkbenchAccountSheetRoute

    @MainActor
    init(
        manager: any WorkbenchAccountManaging,
        route: WorkbenchAccountSheetRoute
    ) {
        self.route = route
        _model = State(
            initialValue: WorkbenchAccountSheetModel(manager: manager)
        )
    }

    var body: some View {
        Group {
            switch route {
            case .add:
                WorkbenchAddAccountView(model: model)
            case .manage:
                WorkbenchAccountManagementSheet(model: model)
            }
        }
        .onAppear {
            model.refresh()
        }
        .onDisappear {
            model.clearTransientState()
        }
    }
}

private struct WorkbenchAddAccountView: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: WorkbenchAccountSheetModel
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Add an account")
                    .font(.title2.weight(.semibold))
                Text(
                    "Use an account key to sign in, or a public profile "
                        + "to browse without signing."
                )
                    .foregroundStyle(.secondary)
            }

            if case .available = model.snapshot.availability {
                identityInput
                if let errorMessage = model.errorMessage {
                    Label(errorMessage, systemImage: "exclamationmark.circle")
                        .font(.callout)
                        .fixedSize(horizontal: false, vertical: true)
                        .accessibilityLabel(errorMessage)
                }
            } else if case let .unavailable(reason) =
                model.snapshot.availability
            {
                ContentUnavailableView(
                    "Accounts aren’t available",
                    systemImage: "person.crop.circle.badge.exclamationmark",
                    description: Text(reason)
                )
            }

            HStack {
                Spacer()
                Button("Cancel", role: .cancel) {
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)

                Button {
                    continueFromInput()
                } label: {
                    if model.isWorking {
                        ProgressView()
                            .controlSize(.small)
                            .accessibilityLabel("Adding account")
                    } else {
                        Text("Continue")
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!canContinue)
                .accessibilityIdentifier("account-add-continue")
                .accessibilityHint("Adds this account and selects it")
            }
        }
        .padding(24)
        #if os(macOS)
        .frame(width: 440)
        #endif
        .task {
            inputFocused = true
        }
    }

    private var identityInput: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Account")
                .font(.callout.weight(.medium))
            inputField

            Text(
                "Private keys stay protected on this Mac. Public profiles "
                    + "open in read-only mode."
            )
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var inputField: some View {
        Group {
            if model.identityLooksSecret {
                SecureField(
                    "Account key or profile address",
                    text: $model.identity
                )
                .textContentType(.password)
                .privacySensitive()
            } else {
                TextField(
                    "Account key or profile address",
                    text: $model.identity
                )
                .textContentType(.username)
            }
        }
        .focused($inputFocused)
        .onSubmit(continueFromInput)
        .accessibilityIdentifier("account-identity")
        .accessibilityLabel("Account")
        .accessibilityHint(
            "Enter an account key to sign in or a public profile to browse."
        )
    }

    private var canContinue: Bool {
        guard !model.isWorking else {
            return false
        }
        return !model.identity.trimmingCharacters(
            in: .whitespacesAndNewlines
        ).isEmpty
    }

    private func continueFromInput() {
        guard canContinue else {
            return
        }
        Task {
            let succeeded = await model.continueWithIdentity()
            if succeeded {
                dismiss()
            } else {
                inputFocused = true
            }
        }
    }
}
