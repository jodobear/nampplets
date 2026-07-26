import SwiftUI

enum WorkbenchAccountSheetRoute: String, Identifiable {
    case secret
    case readOnly
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
            case .secret:
                WorkbenchAddAccountView(model: model, kind: .secret)
            case .readOnly:
                WorkbenchAddAccountView(model: model, kind: .readOnly)
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

private enum WorkbenchAccountInputKind {
    case secret
    case readOnly

    var title: String {
        switch self {
        case .secret:
            "Add an account"
        case .readOnly:
            "Browse as a profile"
        }
    }

    var detail: String {
        switch self {
        case .secret:
            "Use the secret account key you already have."
        case .readOnly:
            "See personalized public activity without signing."
        }
    }
}

private struct WorkbenchAddAccountView: View {
    @Environment(\.dismiss) private var dismiss
    @Bindable var model: WorkbenchAccountSheetModel
    let kind: WorkbenchAccountInputKind
    @FocusState private var inputFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            VStack(alignment: .leading, spacing: 6) {
                Text(kind.title)
                    .font(.title2.weight(.semibold))
                Text(kind.detail)
                    .foregroundStyle(.secondary)
            }

            if case .available = model.snapshot.availability {
                input
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

    @ViewBuilder
    private var input: some View {
        switch kind {
        case .secret:
            VStack(alignment: .leading, spacing: 8) {
                Text("Secret account key")
                    .font(.callout.weight(.medium))
                SecureField(
                    "Paste your secret account key",
                    text: $model.secret
                )
                .textContentType(.password)
                .privacySensitive()
                .focused($inputFocused)
                .onSubmit(continueFromInput)
                .accessibilityIdentifier("account-secret-key")
                .accessibilityLabel("Secret account key")
                .accessibilityHint(
                    "Paste the private key for the account you want to add."
                )

                Text(
                    "Your key stays protected on this Mac and is never shared "
                        + "with napplets."
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            }
        case .readOnly:
            VStack(alignment: .leading, spacing: 8) {
                Text("Profile address")
                    .font(.callout.weight(.medium))
                TextField(
                    "Paste a profile address",
                    text: $model.publicIdentity
                )
                .textContentType(.username)
                .focused($inputFocused)
                .onSubmit(continueFromInput)
                .accessibilityIdentifier("account-profile-address")
                .accessibilityLabel("Profile address")
                .accessibilityHint(
                    "Enter the public profile you want to browse as."
                )

                Text("You can browse, but you can’t post or approve actions.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var canContinue: Bool {
        guard !model.isWorking else {
            return false
        }
        switch kind {
        case .secret:
            return !model.secret.trimmingCharacters(
                in: .whitespacesAndNewlines
            ).isEmpty
        case .readOnly:
            return !model.publicIdentity.trimmingCharacters(
                in: .whitespacesAndNewlines
            ).isEmpty
        }
    }

    private func continueFromInput() {
        guard canContinue else {
            return
        }
        Task {
            let succeeded = switch kind {
            case .secret:
                await model.continueWithSecret()
            case .readOnly:
                await model.continueReadOnly()
            }
            if succeeded {
                dismiss()
            } else {
                inputFocused = true
            }
        }
    }
}
