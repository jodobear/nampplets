import Foundation
import NMPNativeRuntimeApple

@MainActor
public final class RuntimeWorkbenchAccountManager:
    WorkbenchAccountManaging
{
    private let profile: WorkbenchRuntimeProfile
    private var current: WorkbenchAccountSnapshot
    private var nativeHandles: [
        WorkbenchAccountHandle: NativeRuntimeAccountHandle
    ] = [:]

    public init(profile: WorkbenchRuntimeProfile) {
        self.profile = profile
        current = .unavailable(reason: "Opening the account service.")
        apply(profile.native.accountSnapshot())
    }

    public func snapshot() -> WorkbenchAccountSnapshot {
        current
    }

    public func register(secret: String) async {
        apply(profile.native.registerLocalAccount(secretKey: secret))
    }

    public func registerReadOnly(publicIdentity: String) async {
        apply(
            profile.native.registerReadOnlyAccount(
                publicIdentity: publicIdentity
            )
        )
    }

    public func activate(handle: WorkbenchAccountHandle) async {
        guard let native = nativeHandles[handle] else {
            setError("The selected account installation is stale.")
            return
        }
        apply(profile.native.activateLocalAccount(handle: native))
    }

    public func logout() async {
        apply(profile.native.logoutLocalAccount())
    }

    public func remove(handle: WorkbenchAccountHandle) async {
        guard let native = nativeHandles[handle] else {
            setError("The selected account installation is stale.")
            return
        }
        apply(profile.native.removeLocalAccount(handle: native))
    }

    private func apply(_ update: NativeRuntimeAccountUpdate) {
        guard update.accepted, let snapshot = update.snapshot else {
            setError(message(for: update.failure))
            return
        }
        guard snapshot.localAccounts.count <=
            WorkbenchAccountSnapshot.maximumAccountCount
        else {
            current = .unavailable(
                reason: "The runtime returned more accounts than the Workbench can display."
            )
            nativeHandles.removeAll(keepingCapacity: false)
            return
        }

        var handles: [
            WorkbenchAccountHandle: NativeRuntimeAccountHandle
        ] = [:]
        let accounts = snapshot.localAccounts.map { native in
            let handle = WorkbenchAccountHandle(
                opaqueValue: "\(native.installationId):\(native.publicKey)"
            )
            handles[handle] = native
            return WorkbenchStoredAccount(
                handle: handle,
                // The pinned NMP facade projects the canonical hex identity,
                // but not a NIP-19 encoder. Keep this absent rather than
                // hand-rolling protocol semantics in Swift.
                npub: "",
                publicKeyHex: native.publicKey,
                connectionKind: connectionKind(native.kind)
            )
        }
        let activeHandle = snapshot.activePublicKey.flatMap { active in
            accounts.first(where: { $0.publicKeyHex == active })?.handle
        }
        nativeHandles = handles
        let persistenceMessage = profile.native.accountPersistenceIssue()
            .flatMap(\.errorDescription)
        current = WorkbenchAccountSnapshot(
            accounts: accounts,
            activeHandle: activeHandle,
            errorMessage: update.failure.map(message(for:))
                ?? persistenceMessage
        )!
    }

    private func connectionKind(
        _ kind: NativeRuntimeAccountKind
    ) -> WorkbenchAccountConnectionKind {
        switch kind {
        case .localSigner:
            .localSigner
        case .readOnly:
            .readOnly
        }
    }

    private func setError(_ message: String) {
        current = WorkbenchAccountSnapshot(
            availability: current.availability,
            accounts: current.accounts,
            activeHandle: current.activeHandle,
            errorMessage: message
        )!
    }

    private func message(
        for failure: NativeRuntimeAccountFailure?
    ) -> String {
        guard let failure else {
            return "The account operation was refused."
        }
        switch failure {
        case .closed:
            return "The account service is closed."
        case .invalidSecretKey:
            return "The secret key is invalid."
        case .invalidPublicKey:
            return "Enter a valid npub or canonical 64-character hexadecimal public key."
        case .nip05ResolutionUnavailable:
            return "NIP-05 sign-in is not available because the pinned NMP facade cannot resolve NIP-05 identifiers yet. Use an npub or hexadecimal public key."
        case let .capacity(limit):
            return "The account registry is full at \(limit) entries."
        case .instanceExhausted:
            return "The account capability identifier space is exhausted."
        case .staleInstallation:
            return "The selected account installation is stale."
        case let .failed(reason):
            return reason
        }
    }
}

enum RuntimeWorkbenchLayoutStoreError: LocalizedError {
    case refused(String)
    case malformed(String)

    var errorDescription: String? {
        switch self {
        case let .refused(detail), let .malformed(detail):
            detail
        }
    }
}

@MainActor
public final class RuntimeWorkbenchLayoutStore:
    WorkbenchLayoutPersisting
{
    private let profile: WorkbenchRuntimeProfile

    public init(profile: WorkbenchRuntimeProfile) {
        self.profile = profile
    }

    public func loadLayout(
        workspaceID: String
    ) throws -> WorkbenchLayoutSnapshot? {
        let result = profile.native.restoreWorkspaces()
        guard result.accepted else {
            throw RuntimeWorkbenchLayoutStoreError.refused(
                result.refusal?.detail ?? "Workspace restore was refused."
            )
        }
        guard let workspace = result.workspaces.first(where: {
            $0.workspaceId == workspaceID
        }) else {
            return nil
        }
        guard let data = workspace.preferencesJson.data(using: .utf8) else {
            throw RuntimeWorkbenchLayoutStoreError.malformed(
                "Workspace preferences are not UTF-8."
            )
        }
        do {
            return try JSONDecoder().decode(
                WorkbenchLayoutSnapshot.self,
                from: data
            )
        } catch {
            throw RuntimeWorkbenchLayoutStoreError.malformed(
                "Workspace preferences do not match a supported canvas layout."
            )
        }
    }

    public func saveLayout(
        _ snapshot: WorkbenchLayoutSnapshot,
        workspaceID: String
    ) throws {
        try saveLayout(
            snapshot,
            workspaceID: workspaceID,
            retainedReceiptIDs: []
        )
    }

    public func saveLayout(
        _ snapshot: WorkbenchLayoutSnapshot,
        workspaceID: String,
        retainedReceiptIDs: [String]
    ) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        let preferences = try encoder.encode(snapshot)
        guard let preferencesJSON = String(
            data: preferences,
            encoding: .utf8
        ) else {
            throw RuntimeWorkbenchLayoutStoreError.malformed(
                "Workspace preferences could not be encoded as UTF-8."
            )
        }
        let definition = NativeRuntimeWorkspaceDefinition(
            schemaVersion: 1,
            workspaceId: workspaceID,
            // Freeform versus tiling is a bounded presentation preference in
            // preferencesJSON; the pinned native workspace facade exposes only
            // horizontal/vertical structural axes.
            axis: .horizontal,
            slots: snapshot.windows.enumerated().map { index, window in
                nativeSlot(
                    window: window,
                    order: UInt16(index)
                )
            },
            focusedSlotId: snapshot.selectedWindowID?.rawValue,
            activityDrawerVisible: false,
            preferencesJson: preferencesJSON,
            retainedReceiptIds: retainedReceiptIDs
        )
        let result = profile.native.saveWorkspace(definition)
        guard result.accepted else {
            throw RuntimeWorkbenchLayoutStoreError.refused(
                result.refusal?.detail ?? "Workspace save was refused."
            )
        }
    }

    private func nativeSlot(
        window: WorkbenchCanvasWindow,
        order: UInt16
    ) -> NativeRuntimeWorkspaceSlot {
        return NativeRuntimeWorkspaceSlot(
            slotId: window.id.rawValue,
            role: .toolWindow,
            renderer: window.exactBuild == nil ? .native : .legacyNapplet,
            handlerId: window.componentID.rawValue,
            manifestAuthor: window.exactBuild?.manifestAuthor,
            dTag: window.exactBuild?.dTag,
            aggregateHash: window.exactBuild?.aggregateHash,
            bindingParametersJson: "{}",
            navigationJson: "{}",
            visible: true,
            order: order,
            sizePoints: boundedPoint(window.frame.width),
            minimumPoints: boundedPoint(WorkbenchWindowFrame.minimumWidth),
            maximumPoints: boundedPoint(WorkbenchWindowFrame.maximumWidth)
        )
    }

    private func boundedPoint(_ value: Double) -> UInt16 {
        UInt16(min(max(value.rounded(), 1), 4_096))
    }
}
