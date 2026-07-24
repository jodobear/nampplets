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
                connectionKind: .localSigner
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
                "Workspace preferences do not match version 1."
            )
        }
    }

    public func saveLayout(
        _ snapshot: WorkbenchLayoutSnapshot,
        workspaceID: String
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
            axis: .horizontal,
            slots: WorkbenchSlotRole.allCases.enumerated().map {
                index,
                role in
                nativeSlot(
                    role: role,
                    order: UInt16(index),
                    snapshot: snapshot
                )
            },
            focusedSlotId: snapshot.focusedRole?.rawValue,
            activityDrawerVisible: false,
            preferencesJson: preferencesJSON,
            retainedReceiptIds: []
        )
        let result = profile.native.saveWorkspace(definition)
        guard result.accepted else {
            throw RuntimeWorkbenchLayoutStoreError.refused(
                result.refusal?.detail ?? "Workspace save was refused."
            )
        }
    }

    private func nativeSlot(
        role: WorkbenchSlotRole,
        order: UInt16,
        snapshot: WorkbenchLayoutSnapshot
    ) -> NativeRuntimeWorkspaceSlot {
        let assigned = snapshot.assignments[role] == .goodMorning
        let constraints = role.constraints
        let size = snapshot.sizes[role] ?? WorkbenchSlotSize(
            width: constraints.idealWidth,
            height: constraints.idealHeight
        )
        let usesHeight = role == .composer
        return NativeRuntimeWorkspaceSlot(
            slotId: role.rawValue,
            role: nativeRole(role),
            renderer: assigned ? .legacyNapplet : .native,
            handlerId: assigned ? GoodMorningFixture.dTag : "native-\(role.rawValue)",
            manifestAuthor: assigned ? GoodMorningFixture.author : nil,
            dTag: assigned ? GoodMorningFixture.dTag : nil,
            aggregateHash: assigned ? GoodMorningFixture.aggregateHash : nil,
            bindingParametersJson: "{}",
            navigationJson: "{}",
            visible: snapshot.visibleRoles.contains(role),
            order: order,
            sizePoints: boundedPoint(usesHeight ? size.height : size.width),
            minimumPoints: boundedPoint(
                usesHeight
                    ? constraints.minimumHeight
                    : constraints.minimumWidth
            ),
            maximumPoints: boundedPoint(
                usesHeight
                    ? constraints.maximumHeight
                    : constraints.maximumWidth
            )
        )
    }

    private func nativeRole(
        _ role: WorkbenchSlotRole
    ) -> NativeRuntimeWorkspaceRole {
        switch role {
        case .feed: .feed
        case .detail: .detail
        case .composer: .composer
        case .tool: .toolWindow
        }
    }

    private func boundedPoint(_ value: Double) -> UInt16 {
        UInt16(min(max(value.rounded(), 1), 4_096))
    }
}
