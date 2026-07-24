import Foundation
import Observation
import SwiftUI

/// A runtime-owned account identifier.
///
/// The value is intentionally opaque to the Workbench. It is passed back only
/// to the account manager and must never be shown as account identity.
public struct WorkbenchAccountHandle: Hashable, Sendable {
    public let opaqueValue: String

    public init(opaqueValue: String) {
        self.opaqueValue = opaqueValue
    }
}

public enum WorkbenchAccountConnectionKind: String, Equatable, Sendable {
    case localSigner
    case remoteSigner
    case readOnly

    public var title: String {
        switch self {
        case .localSigner: "Local signer"
        case .remoteSigner: "Remote signer"
        case .readOnly: "Read-only"
        }
    }
}

/// A bounded, non-secret account projection intended for native rendering.
public struct WorkbenchStoredAccount: Identifiable, Equatable, Sendable {
    public let handle: WorkbenchAccountHandle
    public let npub: String
    public let publicKeyHex: String
    public let connectionKind: WorkbenchAccountConnectionKind

    public var id: WorkbenchAccountHandle {
        handle
    }

    public init(
        handle: WorkbenchAccountHandle,
        npub: String,
        publicKeyHex: String,
        connectionKind: WorkbenchAccountConnectionKind
    ) {
        self.handle = handle
        self.npub = npub
        self.publicKeyHex = publicKeyHex
        self.connectionKind = connectionKind
    }
}

/// The active account projection. It contains public identity only.
public struct WorkbenchActiveAccount: Equatable, Sendable {
    public let handle: WorkbenchAccountHandle
    public let npub: String
    public let publicKeyHex: String
    public let connectionKind: WorkbenchAccountConnectionKind

    fileprivate init(account: WorkbenchStoredAccount) {
        handle = account.handle
        npub = account.npub
        publicKeyHex = account.publicKeyHex
        connectionKind = account.connectionKind
    }
}

public enum WorkbenchAccountAvailability: Equatable, Sendable {
    case available
    case unavailable(reason: String)
}

/// Screen-shaped account state. The Rust-backed adapter remains its owner.
public struct WorkbenchAccountSnapshot: Equatable, Sendable {
    public static let maximumAccountCount = 32

    public let availability: WorkbenchAccountAvailability
    public let accounts: [WorkbenchStoredAccount]
    public let activeHandle: WorkbenchAccountHandle?
    public let errorMessage: String?

    public var activeAccount: WorkbenchActiveAccount? {
        guard
            let activeHandle,
            let account = accounts.first(where: { $0.handle == activeHandle })
        else {
            return nil
        }
        return WorkbenchActiveAccount(account: account)
    }

    public init?(
        availability: WorkbenchAccountAvailability = .available,
        accounts: [WorkbenchStoredAccount],
        activeHandle: WorkbenchAccountHandle?,
        errorMessage: String? = nil
    ) {
        guard accounts.count <= Self.maximumAccountCount else {
            return nil
        }
        if let activeHandle {
            guard accounts.contains(where: { $0.handle == activeHandle }) else {
                return nil
            }
        }
        self.availability = availability
        self.accounts = accounts
        self.activeHandle = activeHandle
        self.errorMessage = errorMessage
    }

    public static func unavailable(reason: String) -> Self {
        WorkbenchAccountSnapshot(
            availability: .unavailable(reason: reason),
            accounts: [],
            activeHandle: nil
        )!
    }
}

/// Native account capability used by the Workbench account presentation.
///
/// Implementations must treat `secret` as transient secret-bearing input: do
/// not log it, persist it outside the approved signer vault, or include it in
/// an error. Registration and activation are deliberately separate actions.
@MainActor
public protocol WorkbenchAccountManaging: AnyObject {
    func snapshot() -> WorkbenchAccountSnapshot
    func register(secret: String) async
    func registerReadOnly(publicIdentity: String) async
    func activate(handle: WorkbenchAccountHandle) async
    func logout() async
    func remove(handle: WorkbenchAccountHandle) async
}

/// Honest default until the application injects its Rust-backed adapter.
@MainActor
public final class UnavailableWorkbenchAccountManager:
    WorkbenchAccountManaging
{
    public static let defaultReason =
        "Account and signer support is unavailable in this runtime build."

    private let reason: String

    public init(reason: String = defaultReason) {
        self.reason = reason
    }

    public func snapshot() -> WorkbenchAccountSnapshot {
        .unavailable(reason: reason)
    }

    public func register(secret _: String) async {}
    public func registerReadOnly(publicIdentity _: String) async {}
    public func activate(handle _: WorkbenchAccountHandle) async {}
    public func logout() async {}
    public func remove(handle _: WorkbenchAccountHandle) async {}
}

@MainActor
@Observable
final class WorkbenchAccountSheetModel {
    private let manager: any WorkbenchAccountManaging

    private(set) var snapshot: WorkbenchAccountSnapshot
    private(set) var errorMessage: String?
    private(set) var isWorking = false
    var secret = ""
    var publicIdentity = ""

    init(manager: any WorkbenchAccountManaging) {
        self.manager = manager
        snapshot = manager.snapshot()
        errorMessage = snapshot.errorMessage
    }

    func refresh() {
        snapshot = manager.snapshot()
        errorMessage = snapshot.errorMessage
    }

    func register() async {
        let submittedSecret = secret
        secret.removeAll(keepingCapacity: false)
        guard !submittedSecret.isEmpty else {
            errorMessage = "Enter an nsec or 64-character hexadecimal secret."
            return
        }

        await perform(redacting: submittedSecret) {
            await manager.register(secret: submittedSecret)
        }
    }

    func registerReadOnly() async {
        let submittedIdentity = publicIdentity
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !submittedIdentity.isEmpty else {
            errorMessage = "Enter an npub or 64-character hexadecimal public key."
            return
        }
        await perform {
            await manager.registerReadOnly(
                publicIdentity: submittedIdentity
            )
        }
        if errorMessage == nil {
            publicIdentity.removeAll(keepingCapacity: false)
        }
    }

    func activate(_ handle: WorkbenchAccountHandle) async {
        await perform {
            await manager.activate(handle: handle)
        }
    }

    func logout() async {
        await perform {
            await manager.logout()
        }
    }

    func remove(_ handle: WorkbenchAccountHandle) async {
        await perform {
            await manager.remove(handle: handle)
        }
    }

    func clearTransientState() {
        secret.removeAll(keepingCapacity: false)
        publicIdentity.removeAll(keepingCapacity: false)
        errorMessage = nil
    }

    private func perform(
        redacting secret: String? = nil,
        operation: () async -> Void
    ) async {
        guard !isWorking else {
            return
        }
        isWorking = true
        errorMessage = nil
        await operation()
        snapshot = manager.snapshot()
        errorMessage = snapshot.errorMessage
        if
            let secret,
            !secret.isEmpty,
            let errorMessage
        {
            self.errorMessage = errorMessage.replacingOccurrences(
                of: secret,
                with: "••••"
            )
        }
        isWorking = false
    }
}

public struct WorkbenchAccountSheet: View {
    @Environment(\.dismiss) private var dismiss
    @State private var model: WorkbenchAccountSheetModel
    @State private var pendingRemoval: WorkbenchStoredAccount?
    @FocusState private var secretFieldFocused: Bool
    @FocusState private var readOnlyFieldFocused: Bool

    @MainActor
    public init(manager: any WorkbenchAccountManaging) {
        _model = State(
            initialValue: WorkbenchAccountSheetModel(manager: manager)
        )
    }

    public var body: some View {
        @Bindable var model = model

        NavigationStack {
            Form {
                activeAccountSection

                if case .available = model.snapshot.availability {
                    registrationSection(secret: $model.secret)
                    readOnlyRegistrationSection(
                        publicIdentity: $model.publicIdentity
                    )
                    accountsSection
                } else if case let .unavailable(reason) =
                    model.snapshot.availability
                {
                    Section {
                        ContentUnavailableView(
                            "Account support unavailable",
                            systemImage: "person.crop.circle.badge.exclamationmark",
                            description: Text(reason)
                        )
                    }
                }

                if let errorMessage = model.errorMessage {
                    Section {
                        Label(errorMessage, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                            .accessibilityLabel("Account error: \(errorMessage)")
                    }
                }
            }
            .formStyle(.grouped)
            .navigationTitle("Account")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") {
                        model.clearTransientState()
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)
                }
            }
        }
        .frame(minWidth: 560, idealWidth: 620, minHeight: 520)
        .onAppear {
            model.refresh()
        }
        .onDisappear {
            model.clearTransientState()
        }
        .confirmationDialog(
            "Remove this account from the Workbench?",
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
            Text("The signer adapter decides whether vault material can be removed.")
        }
    }

    @ViewBuilder
    private var activeAccountSection: some View {
        Section("Active account") {
            if let account = model.snapshot.activeAccount {
                LabeledContent("Connection", value: account.connectionKind.title)
                LabeledContent("npub") {
                    Text(
                        account.npub.isEmpty
                            ? "Not projected by this runtime"
                            : account.npub
                    )
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                        .accessibilityLabel("Active account npub")
                        .accessibilityValue(
                            account.npub.isEmpty ? "Unavailable" : account.npub
                        )
                }
                LabeledContent("Hex public key") {
                    Text(account.publicKeyHex)
                        .font(.system(.caption, design: .monospaced))
                        .textSelection(.enabled)
                        .accessibilityLabel("Active account hexadecimal public key")
                        .accessibilityValue(account.publicKeyHex)
                }
                Button("Log Out", role: .destructive) {
                    Task {
                        await model.logout()
                    }
                }
                .disabled(model.isWorking)
                .keyboardShortcut("l", modifiers: [.command, .shift])
                .accessibilityHint(
                    "Signs out while keeping the registered account available"
                )
            } else {
                Label("Signed out", systemImage: "person.crop.circle.badge.xmark")
                    .foregroundStyle(.secondary)
                    .accessibilityLabel("No active account")
            }
        }
    }

    private func registrationSection(
        secret: Binding<String>
    ) -> some View {
        Section {
            SecureField(
                "nsec or 64-character hex secret",
                text: secret
            )
            .textContentType(.password)
            .privacySensitive()
            .focused($secretFieldFocused)
            .onSubmit {
                Task {
                    await model.register()
                    secretFieldFocused = model.errorMessage != nil
                }
            }
            .accessibilityLabel("Secret key")
            .accessibilityHint(
                "Enter an nsec or hexadecimal secret. It is never displayed."
            )

            Button("Register Local Account") {
                Task {
                    await model.register()
                    secretFieldFocused = model.errorMessage != nil
                }
            }
            .disabled(model.isWorking || model.secret.isEmpty)
            .keyboardShortcut(.defaultAction)
            .accessibilityHint(
                "Registers the signer without making it the active account"
            )
        } header: {
            Text("Register local signer")
        } footer: {
            Text("Registration and activation are separate. Secrets never enter a napplet.")
        }
    }

    private func readOnlyRegistrationSection(
        publicIdentity: Binding<String>
    ) -> some View {
        Section {
            TextField(
                "npub or 64-character hex public key",
                text: publicIdentity
            )
            .textContentType(.username)
            .focused($readOnlyFieldFocused)
            .onSubmit {
                Task {
                    await model.registerReadOnly()
                    readOnlyFieldFocused = model.errorMessage != nil
                }
            }
            .accessibilityLabel("Read-only account public identity")
            .accessibilityHint(
                "Enter an npub or hexadecimal public key. NIP-05 resolution is unavailable in this pinned runtime."
            )

            Button("Add Read-Only Account") {
                Task {
                    await model.registerReadOnly()
                    readOnlyFieldFocused = model.errorMessage != nil
                }
            }
            .disabled(
                model.isWorking
                    || model.publicIdentity
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                        .isEmpty
            )
            .accessibilityHint(
                "Adds the keyless account without making it active"
            )
        } header: {
            Text("Add read-only account")
        } footer: {
            Text(
                "Read-only accounts can browse as an npub without a signer. NIP-05 resolution will be enabled when the pinned NMP facade supports it."
            )
        }
    }

    @ViewBuilder
    private var accountsSection: some View {
        Section("Registered accounts") {
            if model.snapshot.accounts.isEmpty {
                Text("No registered accounts")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(model.snapshot.accounts) { account in
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(alignment: .firstTextBaseline) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(
                                    account.npub.isEmpty
                                        ? account.publicKeyHex
                                        : account.npub
                                )
                                    .font(.system(.body, design: .monospaced))
                                    .lineLimit(1)
                                    .textSelection(.enabled)
                                Text(account.connectionKind.title)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            if account.handle == model.snapshot.activeHandle {
                                Label("Active", systemImage: "checkmark.circle.fill")
                                    .foregroundStyle(.green)
                            }
                        }

                        HStack {
                            if account.handle != model.snapshot.activeHandle {
                                Button("Activate") {
                                    Task {
                                        await model.activate(account.handle)
                                    }
                                }
                                .disabled(model.isWorking)
                                .accessibilityLabel(
                                    "Activate \(account.npub.isEmpty ? account.publicKeyHex : account.npub)"
                                )
                            }

                            Spacer()

                            Button("Remove", role: .destructive) {
                                pendingRemoval = account
                            }
                            .disabled(model.isWorking)
                            .accessibilityLabel(
                                "Remove \(account.npub.isEmpty ? account.publicKeyHex : account.npub)"
                            )
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
        }
    }
}
