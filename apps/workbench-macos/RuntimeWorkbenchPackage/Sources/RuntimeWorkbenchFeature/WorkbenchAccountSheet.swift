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
    @State private var revealsIdentity = false

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

            if model.requiresIdentityUseChoice {
                identityUseChoice
            }
        }
    }

    private var identityUseChoice: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("How do you use this key?")
                .font(.callout.weight(.medium))
            Text(
                "Is this a key you sign with, or one you only want to watch?"
            )
            .font(.caption)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)

            Picker(
                "How do you use this key?",
                selection: $model.ambiguousIdentityUse
            ) {
                Text("Sign and publish")
                    .accessibilityLabel("Sign and publish")
                    .accessibilityHint(
                        "Uses this key to publish and sign on your behalf"
                    )
                    .accessibilityIdentifier("account-use-signing")
                    .tag(WorkbenchAccountIdentityUse?.some(.signing))
                Text("Browse only")
                    .accessibilityLabel("Browse only")
                    .accessibilityHint(
                        "Uses this identity without signing or publishing"
                    )
                    .accessibilityIdentifier("account-use-read-only")
                    .tag(WorkbenchAccountIdentityUse?.some(.readOnly))
            }
            .labelsHidden()
            .pickerStyle(.segmented)
        }
        .padding(.top, 4)
    }

    @ViewBuilder
    private var inputField: some View {
        HStack(spacing: 8) {
            Group {
                if revealsIdentity {
                    TextField(
                        "Account key or profile address",
                        text: $model.identity
                    )
                    .textContentType(.username)
                } else {
                    SecureField(
                        "Account key or profile address",
                        text: $model.identity
                    )
                    .textContentType(.password)
                }
            }
            .focused($inputFocused)
            .onSubmit(continueFromInput)
            .accessibilityIdentifier("account-identity")
            .accessibilityLabel("Account")
            .accessibilityHint(
                "Enter an account key to sign in or a public profile to browse."
            )

            Button {
                revealsIdentity.toggle()
                inputFocused = true
            } label: {
                Image(systemName: revealsIdentity ? "eye.slash" : "eye")
            }
            .buttonStyle(.borderless)
            .help(revealsIdentity ? "Hide account" : "Show account")
            .accessibilityLabel(
                revealsIdentity ? "Hide account" : "Show account"
            )
        }
        .privacySensitive()
    }

    private var canContinue: Bool {
        guard !model.isWorking else {
            return false
        }
        let hasIdentity = !model.identity.trimmingCharacters(
            in: .whitespacesAndNewlines
        ).isEmpty
        let hasRequiredChoice = !model.requiresIdentityUseChoice
            || model.ambiguousIdentityUse != nil
        return hasIdentity && hasRequiredChoice
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
