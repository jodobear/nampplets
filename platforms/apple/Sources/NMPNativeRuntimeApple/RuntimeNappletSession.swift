import Foundation
import NMPNativeRuntime

public enum RuntimeNappletOpenError: Error, LocalizedError, Equatable {
    case invalidStorageRoot
    case artifactSourceRefused(detail: String)
    case artifactRefused(code: String, detail: String)
    case installRefused(detail: String)
    case installedArtifactProfileMismatch
    case launchRefused(detail: String)
    case observerRefused(code: String, detail: String)
    case invalidAccountPersistence

    public var errorDescription: String? {
        switch self {
        case .invalidStorageRoot:
            "The native runtime storage directory is unavailable."
        case let .artifactSourceRefused(detail):
            "The native artifact source was refused: \(detail)"
        case let .artifactRefused(code, detail):
            "Artifact verification was refused (\(code)): \(detail)"
        case let .installRefused(detail):
            "The native runtime refused to install the artifact: \(detail)"
        case .installedArtifactProfileMismatch:
            "The installed artifact belongs to a different runtime profile."
        case let .launchRefused(detail):
            "The native runtime refused to launch the artifact: \(detail)"
        case let .observerRefused(code, detail):
            "Runtime observation was refused (\(code)): \(detail)"
        case .invalidAccountPersistence:
            "The native account persistence configuration is invalid."
        }
    }
}

/// Immutable bytes supplied to Rust's signed-manifest resolver.
///
/// The callback does not decide whether a URL, digest, redirect, size, or
/// response is acceptable. It reports only the bytes and source selected by
/// the Rust-owned request; Rust revalidates every fact before sealing them.
private final class RegisteredArtifactSource: ArtifactSource, @unchecked Sendable {
    private struct Entry {
        let bytes: Data
        var references: Int
    }

    private struct Registration {
        let digests: [String]
    }

    private static let maximumRegisteredBlobs = 256
    private static let maximumRegisteredBytes = 32 * 1_024 * 1_024

    private let lock = NSLock()
    private var entries: [String: Entry] = [:]
    private var registrations: [UUID: Registration] = [:]
    private var totalBytes = 0

    func register(_ blobsByDigest: [String: Data]) throws -> UUID {
        guard blobsByDigest.count <= Self.maximumRegisteredBlobs else {
            throw RuntimeNappletOpenError.artifactSourceRefused(
                detail: "A registration may contain at most \(Self.maximumRegisteredBlobs) blobs"
            )
        }
        let lowercaseHex = CharacterSet(charactersIn: "0123456789abcdef")
        for (digest, bytes) in blobsByDigest {
            guard digest.utf8.count == 64,
                  digest.unicodeScalars.allSatisfy(lowercaseHex.contains),
                  !bytes.isEmpty
            else {
                throw RuntimeNappletOpenError.artifactSourceRefused(
                    detail: "Every registered blob needs a lowercase SHA-256 digest and bytes"
                )
            }
        }

        lock.lock()
        defer { lock.unlock() }

        var additionalBytes = 0
        for (digest, bytes) in blobsByDigest {
            if let existing = entries[digest] {
                guard existing.bytes == bytes else {
                    throw RuntimeNappletOpenError.artifactSourceRefused(
                        detail: "Conflicting bytes were registered for digest \(digest)"
                    )
                }
            } else {
                additionalBytes += bytes.count
            }
        }
        let additionalCount = blobsByDigest.keys.filter { entries[$0] == nil }.count
        guard entries.count + additionalCount <= Self.maximumRegisteredBlobs,
              totalBytes + additionalBytes <= Self.maximumRegisteredBytes
        else {
            throw RuntimeNappletOpenError.artifactSourceRefused(
                detail: "The profile artifact source reached its finite registration limit"
            )
        }

        let token = UUID()
        for (digest, bytes) in blobsByDigest {
            if var existing = entries[digest] {
                existing.references += 1
                entries[digest] = existing
            } else {
                entries[digest] = Entry(bytes: bytes, references: 1)
                totalBytes += bytes.count
            }
        }
        registrations[token] = Registration(digests: blobsByDigest.keys.sorted())
        return token
    }

    func unregister(_ token: UUID) {
        lock.lock()
        defer { lock.unlock() }
        guard let registration = registrations.removeValue(forKey: token) else {
            return
        }
        for digest in registration.digests {
            guard var entry = entries[digest] else { continue }
            entry.references -= 1
            if entry.references == 0 {
                totalBytes -= entry.bytes.count
                entries.removeValue(forKey: digest)
            } else {
                entries[digest] = entry
            }
        }
    }

    func fetch(request: ArtifactFetchRequest) -> ArtifactFetchResponse {
        lock.lock()
        let bytes = entries[request.expectedSha256]?.bytes
        lock.unlock()
        guard let bytes else {
            return .refused(reason: "No bundled bytes match the requested digest")
        }
        guard let sourceURL = request.candidateUrls.first else {
            return .refused(reason: "The verified manifest has no candidate source")
        }
        return .body(sourceUrl: sourceURL, httpStatus: 200, bytes: bytes)
    }
}

public enum NativeRuntimeAccountPersistence: Equatable, Sendable {
    /// Local credentials live only for this profile process.
    case transient
    /// Local credentials are stored in a profile-scoped macOS Keychain
    /// namespace. The namespace is hashed before becoming a service name.
    case keychain(namespace: String)
}

public enum NativeRuntimeAccountPersistenceIssue:
    String,
    Error,
    LocalizedError,
    Equatable,
    Sendable
{
    case restoreFailed
    case registerFailed
    case activationFailed
    case logoutFailed
    case removalFailed

    public var errorDescription: String? {
        switch self {
        case .restoreFailed:
            "Saved accounts could not be restored securely."
        case .registerFailed:
            "The account is available for this session but was not saved securely."
        case .activationFailed:
            "The active account changed for this session but was not saved securely."
        case .logoutFailed:
            "The account is logged out for this session but secure persistence was not updated."
        case .removalFailed:
            "The account was removed for this session but secure persistence was not fully updated."
        }
    }
}

public struct NativeRuntimeProfileConfiguration: Sendable {
    public let storageRoot: URL
    public let indexerRelays: [String]
    public let appRelays: [String]
    public let fallbackRelays: [String]
    public let allowedLocalRelayHosts: [String]
    public let accountPersistence: NativeRuntimeAccountPersistence
    public let permissionMode: NativeRuntimePermissionMode

    public init(
        storageRoot: URL,
        indexerRelays: [String] = [],
        appRelays: [String] = [],
        fallbackRelays: [String] = [],
        allowedLocalRelayHosts: [String] = [],
        accountPersistence: NativeRuntimeAccountPersistence = .transient,
        permissionMode: NativeRuntimePermissionMode = .interactive
    ) {
        self.storageRoot = storageRoot
        self.indexerRelays = indexerRelays
        self.appRelays = appRelays
        self.fallbackRelays = fallbackRelays
        self.allowedLocalRelayHosts = allowedLocalRelayHosts
        self.accountPersistence = accountPersistence
        self.permissionMode = permissionMode
    }
}

/// One immutable verified artifact installed into exactly one runtime profile.
///
/// The Rust handle remains opaque. Native callers can use the exact coordinate
/// for review presentation, but cannot replace its bytes, requirements, or
/// launch authority.
public final class NativeRuntimeInstalledArtifact: @unchecked Sendable {
    public let title: String
    public let permissionCoordinate: NativeRuntimePermissionCoordinate

    fileprivate let ownerID: UUID
    fileprivate let artifact: VerifiedArtifact

    fileprivate init(
        title: String,
        ownerID: UUID,
        artifact: VerifiedArtifact,
        permissionCoordinate: NativeRuntimePermissionCoordinate
    ) {
        self.title = title
        self.ownerID = ownerID
        self.artifact = artifact
        self.permissionCoordinate = permissionCoordinate
    }
}

/// One catalog-confirmed exact build installed into this profile.
///
/// The opaque artifact remains profile-bound and is retained only so the app
/// may perform the separate permission and launch steps later.
public struct NativeRuntimeCatalogInstallation: @unchecked Sendable {
    public let title: String
    public let manifestAuthor: String
    public let dTag: String
    public let aggregateHash: String
    public let installedArtifact: NativeRuntimeInstalledArtifact
}

public enum NativeRuntimeCatalogInstallResult: @unchecked Sendable {
    case installed(NativeRuntimeCatalogInstallation)
    case refused(NativeRuntimeCatalogFailure)
}

/// Replacement semantics for the profile-owned permanent NMP catalog feed.
public enum NativeRuntimeCatalogUpdate: Sendable {
    case authoritative(NativeRuntimeCatalogFeedSnapshot)
    case next(
        NativeRuntimeCatalogFeedSnapshot,
        predecessorRevision: UInt64
    )
}

public enum NativeRuntimeCatalogObservationError: Error, Equatable, Sendable {
    case profileClosed
    case observerCapacity(maximum: Int)
}

/// Idempotent application-observer cancellation. Cancelling this fanout does
/// not stop the profile-owned NMP subscription.
public final class NativeRuntimeCatalogObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancellation: (@Sendable () -> Void)?

    fileprivate init(cancellation: @escaping @Sendable () -> Void) {
        self.cancellation = cancellation
    }

    public func cancel() {
        lock.lock()
        let cancellation = cancellation
        self.cancellation = nil
        lock.unlock()
        cancellation?()
    }

    deinit {
        cancel()
    }
}

/// Exact-build identity attached by the Rust runtime to activity facts.
public struct NativeRuntimeActivityScope: Hashable, Sendable {
    public let manifestAuthor: String
    public let dTag: String
    public let aggregateHash: String

    public init(
        manifestAuthor: String,
        dTag: String,
        aggregateHash: String
    ) {
        self.manifestAuthor = manifestAuthor
        self.dTag = dTag
        self.aggregateHash = aggregateHash
    }
}

/// A persisted, runtime-owned activity fact. Native code receives the
/// classification strings verbatim and does not become an activity store.
public struct NativeRuntimeActivityRecord: Sendable {
    public let scope: NativeRuntimeActivityScope
    public let category: String
    public let operation: String
    public let outcome: String
    public let occurredAtMillis: UInt64

    fileprivate init(_ record: RuntimeActivitySnapshot) {
        scope = NativeRuntimeActivityScope(
            manifestAuthor: record.author,
            dTag: record.dTag,
            aggregateHash: record.aggregateHash
        )
        category = record.category
        operation = record.operation
        outcome = record.outcome
        occurredAtMillis = record.occurredAtMillis
    }
}

/// A runtime-owned refusal or failure attributed to one exact build.
///
/// Errors without a complete principal remain absent from the per-component
/// view so native presentation cannot leak unrelated profile activity.
public struct NativeRuntimeActivityError: Sendable {
    public let scope: NativeRuntimeActivityScope
    public let code: String
    public let sessionID: UInt64?
    public let detail: String
    public let occurredAtMillis: UInt64

    fileprivate init?(_ error: RuntimeErrorSnapshot) {
        guard let author = error.author,
              let dTag = error.dTag,
              let aggregateHash = error.aggregateHash
        else {
            return nil
        }
        scope = NativeRuntimeActivityScope(
            manifestAuthor: author,
            dTag: dTag,
            aggregateHash: aggregateHash
        )
        code = error.code
        sessionID = error.sessionId
        detail = error.detail
        occurredAtMillis = error.occurredAtMillis
    }
}

public struct NativeRuntimeActivitySession: Sendable {
    public let scope: NativeRuntimeActivityScope
    public let sessionID: UInt64
    public let state: String

    fileprivate init(_ session: RuntimeSessionSnapshot) {
        scope = NativeRuntimeActivityScope(
            manifestAuthor: session.author,
            dTag: session.dTag,
            aggregateHash: session.aggregateHash
        )
        sessionID = session.id
        state = session.state
    }
}

/// A Rust-retained provider write awaiting one explicit native decision.
/// The draft is display-only; native cannot replace the retained write.
public struct NativeRuntimePendingWrite: Sendable, Identifiable {
    public let id: UInt64
    public let approvalID: String
    public let scope: NativeRuntimeActivityScope
    public let sessionID: UInt64
    public let account: String
    public let draftJSON: String

    fileprivate init(_ pending: RuntimePendingWriteSnapshot) {
        id = pending.operationId
        approvalID = pending.approvalId
        scope = NativeRuntimeActivityScope(
            manifestAuthor: pending.author,
            dTag: pending.dTag,
            aggregateHash: pending.aggregateHash
        )
        sessionID = pending.sessionId
        account = pending.account
        draftJSON = pending.draftJson
    }
}

public struct NativeRuntimePendingWriteProjection: Sendable {
    public let revision: UInt64
    public let writes: [NativeRuntimePendingWrite]

    fileprivate init(_ snapshot: RuntimeSnapshot) {
        revision = snapshot.revision
        writes = snapshot.pendingWrites.map(NativeRuntimePendingWrite.init)
    }
}

public enum NativeRuntimePendingWriteUpdate: Sendable {
    case authoritative(NativeRuntimePendingWriteProjection)
    case next(
        NativeRuntimePendingWriteProjection,
        predecessorRevision: UInt64,
        eventCursorWasStale: Bool
    )
}

public enum NativeRuntimePendingWriteObservationError:
    Error,
    LocalizedError,
    Equatable
{
    case profileClosed
    case observerCapacity(maximum: Int)

    public var errorDescription: String? {
        switch self {
        case .profileClosed:
            "The native runtime profile is closed."
        case let .observerCapacity(maximum):
            "The native pending-write observer limit of \(maximum) was reached."
        }
    }
}

public final class NativeRuntimePendingWriteObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancellation: (@Sendable () -> Void)?

    fileprivate init(cancellation: @escaping @Sendable () -> Void) {
        self.cancellation = cancellation
    }

    public func cancel() {
        lock.lock()
        let cancellation = cancellation
        self.cancellation = nil
        lock.unlock()
        cancellation?()
    }

    deinit {
        cancel()
    }
}

/// A durable NMP receipt mechanically projected for native presentation.
/// NMP remains the sole owner of delivery state and canonical event rows.
public struct NativeRuntimeReceipt: Sendable, Identifiable {
    public let id: String
    public let delivery: String
    public let latestStateJSON: String?

    fileprivate init(_ receipt: RuntimeReceiptSnapshot) {
        id = receipt.receiptId
        delivery = receipt.delivery
        latestStateJSON = receipt.latestStateJson
    }
}

public struct NativeRuntimeReceiptProjection: Sendable {
    public let revision: UInt64
    public let receipts: [NativeRuntimeReceipt]

    fileprivate init(_ snapshot: RuntimeSnapshot) {
        revision = snapshot.revision
        receipts = snapshot.receipts.map(NativeRuntimeReceipt.init)
    }
}

public enum NativeRuntimeReceiptUpdate: Sendable {
    case authoritative(NativeRuntimeReceiptProjection)
    case next(
        NativeRuntimeReceiptProjection,
        predecessorRevision: UInt64,
        eventCursorWasStale: Bool
    )
}

public enum NativeRuntimeReceiptObservationError:
    Error,
    LocalizedError,
    Equatable
{
    case profileClosed
    case observerCapacity(maximum: Int)

    public var errorDescription: String? {
        switch self {
        case .profileClosed:
            "The native runtime profile is closed."
        case let .observerCapacity(maximum):
            "The native receipt observer limit of \(maximum) was reached."
        }
    }
}

public final class NativeRuntimeReceiptObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancellation: (@Sendable () -> Void)?

    fileprivate init(cancellation: @escaping @Sendable () -> Void) {
        self.cancellation = cancellation
    }

    public func cancel() {
        lock.lock()
        let cancellation = cancellation
        self.cancellation = nil
        lock.unlock()
        cancellation?()
    }

    deinit {
        cancel()
    }
}

/// A bounded replacement projection sourced from the Rust runtime.
///
/// Bindings, receipts, and resources are intentionally not projected here:
/// their current FFI records lack exact-build ownership, so exposing the
/// profile-global totals in a component-scoped drawer would disclose unrelated
/// activity.
public struct NativeRuntimeActivityProjection: Sendable {
    public let revision: UInt64
    public let sessions: [NativeRuntimeActivitySession]
    public let records: [NativeRuntimeActivityRecord]
    public let errors: [NativeRuntimeActivityError]

    fileprivate init(
        _ snapshot: RuntimeSnapshot,
        scope: NativeRuntimeActivityScope
    ) {
        revision = snapshot.revision
        sessions = snapshot.sessions
            .map(NativeRuntimeActivitySession.init)
            .filter { $0.scope == scope }
        records = snapshot.recentActivity
            .map(NativeRuntimeActivityRecord.init)
            .filter { $0.scope == scope }
        errors = snapshot.recentErrors
            .compactMap(NativeRuntimeActivityError.init)
            .filter { $0.scope == scope }
    }
}

/// Pushed replacement semantics for application-owned native presentation.
public enum NativeRuntimeActivityUpdate: Sendable {
    case authoritative(NativeRuntimeActivityProjection)
    case next(
        NativeRuntimeActivityProjection,
        predecessorRevision: UInt64,
        eventCursorWasStale: Bool
    )
}

public enum NativeRuntimeActivityObservationError:
    Error,
    LocalizedError,
    Equatable
{
    case profileClosed
    case observerCapacity(maximum: Int)

    public var errorDescription: String? {
        switch self {
        case .profileClosed:
            "The native runtime profile is closed."
        case let .observerCapacity(maximum):
            "The native activity observer limit of \(maximum) was reached."
        }
    }
}

/// Cancellation handle for one application observer.
public final class NativeRuntimeActivityObservation: @unchecked Sendable {
    private let lock = NSLock()
    private var cancellation: (@Sendable () -> Void)?

    fileprivate init(cancellation: @escaping @Sendable () -> Void) {
        self.cancellation = cancellation
    }

    public func cancel() {
        lock.lock()
        let cancellation = cancellation
        self.cancellation = nil
        lock.unlock()
        cancellation?()
    }

    deinit {
        cancel()
    }
}

/// One application trust profile owns exactly one Rust runtime controller,
/// NMP engine, runtime store, artifact cache, and observation stream.
///
/// Napplet sessions borrow this profile. Stopping or crashing one session
/// cannot close the profile or terminate sibling sessions.
public final class NativeRuntimeProfile: RuntimeObserver, @unchecked Sendable {
    private typealias ActivityReceiver =
        @Sendable (NativeRuntimeActivityUpdate) -> Void
    private typealias LibraryReceiver =
        @Sendable (NativeRuntimeLibraryUpdate) -> Void
    private typealias CatalogReceiver =
        @Sendable (NativeRuntimeCatalogUpdate) -> Void
    private typealias PendingWriteReceiver =
        @Sendable (NativeRuntimePendingWriteUpdate) -> Void
    private typealias ReceiptReceiver =
        @Sendable (NativeRuntimeReceiptUpdate) -> Void

    private struct ActivityObserverEntry {
        let scope: NativeRuntimeActivityScope
        let receive: ActivityReceiver
    }

    private struct LibraryObserverEntry {
        let receive: LibraryReceiver
        var lastDeliveredRevision: UInt64
        var isReadyForNext = false
        var pendingUpdate: NativeRuntimeLibraryUpdate?
    }

    private struct CatalogObserverEntry {
        let receive: CatalogReceiver
        var lastDeliveredRevision: UInt64
        var isReadyForNext = false
        var pendingUpdate: NativeRuntimeCatalogUpdate?
    }

    private struct PendingWriteObserverEntry {
        let receive: PendingWriteReceiver
        var lastDeliveredRevision: UInt64
        var isReadyForNext = false
        var pendingUpdate: NativeRuntimePendingWriteUpdate?
    }

    private struct ReceiptObserverEntry {
        let receive: ReceiptReceiver
        var lastDeliveredRevision: UInt64
        var isReadyForNext = false
        var pendingUpdate: NativeRuntimeReceiptUpdate?
    }

    private final class WeakSession {
        weak var value: RustRuntimeNappletSession?

        init(_ value: RustRuntimeNappletSession) {
            self.value = value
        }
    }

    private static let maximumReadBytes: UInt64 = 8 * 1_024 * 1_024
    private static let maximumApplicationActivityObservers = 8
    private static let maximumApplicationLibraryObservers = 8
    private static let maximumApplicationCatalogObservers = 8
    private static let maximumApplicationPendingWriteObservers = 8
    private static let maximumApplicationReceiptObservers = 8
    private static let maximumAccounts = 32

    private let profileID = UUID()
    private let controller: RuntimeController
    private let source: RegisteredArtifactSource
    private let appearanceSource: MacOSAppearanceSource
    private let settingsExecutor: MacOSSettingsExecutor
    private let incActionExecutor: MacOSIncActionExecutor
    private let accountVault: (any NativeAccountVault)?
    private let lock = NSLock()
    private let accountLock = NSLock()
    private var observation: RuntimeObservation?
    private var sessions: [UInt64: WeakSession] = [:]
    private var activityObservers: [UUID: ActivityObserverEntry] = [:]
    private var libraryObservers: [UUID: LibraryObserverEntry] = [:]
    private var catalogObservers: [UUID: CatalogObserverEntry] = [:]
    private var pendingWriteObservers: [UUID: PendingWriteObserverEntry] = [:]
    private var receiptObservers: [UUID: ReceiptObserverEntry] = [:]
    private var lastActivityRevision: UInt64
    private var lastLibraryRevision: UInt64
    private var lastCatalogSnapshot: NativeRuntimeCatalogFeedSnapshot
    private var lastPendingWriteRevision: UInt64
    private var lastReceiptRevision: UInt64
    private var accountPersistenceProblem:
        NativeRuntimeAccountPersistenceIssue?
    private var isClosed = false

    public static func open(
        configuration: NativeRuntimeProfileConfiguration
    ) throws -> NativeRuntimeProfile {
        try open(configuration: configuration, accountVault: nil)
    }

    static func open(
        configuration: NativeRuntimeProfileConfiguration,
        accountVault injectedAccountVault: (any NativeAccountVault)?
    ) throws -> NativeRuntimeProfile {
        do {
            try FileManager.default.createDirectory(
                at: configuration.storageRoot,
                withIntermediateDirectories: true
            )
        } catch {
            throw RuntimeNappletOpenError.invalidStorageRoot
        }

        let source = RegisteredArtifactSource()
        let appearanceSource = MacOSAppearanceSource()
        let settingsExecutor = MacOSSettingsExecutor()
        let incActionExecutor = MacOSIncActionExecutor()
        let accountVault: (any NativeAccountVault)?
        if let injectedAccountVault {
            accountVault = injectedAccountVault
        } else {
            switch configuration.accountPersistence {
            case .transient:
                accountVault = nil
            case let .keychain(namespace):
                do {
                    accountVault = try MacOSKeychainAccountVault(
                        namespace: namespace
                    )
                } catch {
                    throw RuntimeNappletOpenError.invalidAccountPersistence
                }
            }
        }
        let controller = try RuntimeController.openWithAllNativeCapabilities(
            config: RuntimeConfig(
                runtimeStorePath: configuration.storageRoot
                    .appendingPathComponent("runtime.sqlite3")
                    .path,
                nmpStorePath: configuration.storageRoot
                    .appendingPathComponent("nmp.redb")
                    .path,
                artifactCachePath: configuration.storageRoot
                    .appendingPathComponent("artifacts", isDirectory: true)
                    .path,
                indexerRelays: configuration.indexerRelays,
                appRelays: configuration.appRelays,
                fallbackRelays: configuration.fallbackRelays,
                allowedLocalRelayHosts: configuration.allowedLocalRelayHosts,
                maximumNmpRelays: 64,
                maximumBridgeWorkers: 12,
                maximumObservers: 4,
                maximumBoundaryEvents: 256,
                maximumConfigItems: 64,
                maximumConfigStringBytes: 16_384,
                maximumManifestBytes: 262_144,
                maximumArtifactFiles: 256,
                maximumArtifactFileBytes: Self.maximumReadBytes,
                maximumArtifactTotalBytes: 32 * 1_024 * 1_024,
                maximumVerifiedReadBytes: Self.maximumReadBytes,
                maximumBlobSources: 8,
                permissionMode: configuration.permissionMode
            ),
            artifactSource: source,
            appearanceSource: appearanceSource,
            settingsExecutor: settingsExecutor,
            incActionExecutor: incActionExecutor
        )
        appearanceSource.bind(controller: controller)
        settingsExecutor.bind(controller: controller)
        let profile = NativeRuntimeProfile(
            controller: controller,
            source: source,
            appearanceSource: appearanceSource,
            settingsExecutor: settingsExecutor,
            incActionExecutor: incActionExecutor,
            accountVault: accountVault
        )
        profile.restorePersistedAccounts()
        do {
            try profile.startObservation()
        } catch {
            controller.close()
            throw error
        }
        return profile
    }

    private init(
        controller: RuntimeController,
        source: RegisteredArtifactSource,
        appearanceSource: MacOSAppearanceSource,
        settingsExecutor: MacOSSettingsExecutor,
        incActionExecutor: MacOSIncActionExecutor,
        accountVault: (any NativeAccountVault)?
    ) {
        self.controller = controller
        self.source = source
        self.appearanceSource = appearanceSource
        self.settingsExecutor = settingsExecutor
        self.incActionExecutor = incActionExecutor
        self.accountVault = accountVault
        let revision = controller.snapshot().revision
        lastActivityRevision = revision
        lastLibraryRevision = revision
        lastCatalogSnapshot = controller.catalogFeedSnapshot()
        lastPendingWriteRevision = revision
        lastReceiptRevision = revision
        accountPersistenceProblem = nil
    }

    private func startObservation() throws {
        let start = controller.observe(observer: self)
        if let refusal = start.refusal {
            throw RuntimeNappletOpenError.observerRefused(
                code: refusal.code,
                detail: refusal.detail
            )
        }
        guard let observation = start.observation else {
            throw RuntimeNappletOpenError.observerRefused(
                code: "missing-observation",
                detail: "The controller admitted no observation handle"
            )
        }
        lock.lock()
        self.observation = observation
        lock.unlock()
    }

    /// Opens one finite, source-scoped NMP catalog projection. This call is
    /// blocking and must be invoked away from the main actor.
    public func browseCatalog(
        query: String
    ) -> NativeRuntimeCatalogPageResult {
        controller.catalogBrowse(query: query)
    }

    /// Freezes one exact signed review from the current bounded catalog page.
    /// This call is blocking and must be invoked away from the main actor.
    public func reviewCatalogEntry(
        eventID: String
    ) -> NativeRuntimeCatalogReviewResult {
        controller.catalogReviewEntry(eventId: eventID)
    }

    /// Resolves a manually entered manifest coordinate exclusively in Rust.
    /// This call is blocking and must be invoked away from the main actor.
    public func reviewCatalogCoordinate(
        _ coordinate: String
    ) -> NativeRuntimeCatalogReviewResult {
        controller.catalogReviewManual(coordinate: coordinate)
    }

    /// Wakes every blocking catalog observation or acquisition.
    @discardableResult
    public func cancelPendingCatalogWork()
        -> NativeRuntimeCatalogCancellationResult
    {
        controller.catalogCancelPending()
    }

    /// Cancels and discards one frozen review without installing it.
    @discardableResult
    public func cancelCatalogReview(
        token: String
    ) -> NativeRuntimeCatalogCancellationResult {
        controller.catalogCancelReview(token: token)
    }

    /// Confirms and installs one frozen exact review. Permission grants and
    /// launch remain separate operations.
    ///
    /// This call is blocking and must be invoked away from the main actor.
    public func confirmCatalogInstall(
        token: String,
        expectedAuthor: String,
        expectedDTag: String,
        expectedAggregateHash: String
    ) -> NativeRuntimeCatalogInstallResult {
        let result = controller.catalogConfirmInstall(
            token: token,
            expectedAuthor: expectedAuthor,
            expectedDTag: expectedDTag,
            expectedAggregateHash: expectedAggregateHash
        )
        if let failure = result.failure {
            return .refused(failure)
        }
        guard
            let confirmation = result.confirmation,
            let artifact = result.artifact,
            let dTag = confirmation.dTag,
            confirmation.manifestAuthor == expectedAuthor,
            dTag == expectedDTag,
            confirmation.aggregateHash == expectedAggregateHash
        else {
            return .refused(
                NativeRuntimeCatalogFailure(
                    code: "incomplete-confirmation",
                    detail: "Rust returned no complete exact catalog installation",
                    provenance: []
                )
            )
        }
        let title = confirmation.title ?? "Untitled napplet"
        let coordinate = NativeRuntimePermissionCoordinate(
            manifestAuthor: confirmation.manifestAuthor,
            dTag: dTag,
            aggregateHash: confirmation.aggregateHash
        )
        let installedArtifact = NativeRuntimeInstalledArtifact(
            title: title,
            ownerID: profileID,
            artifact: artifact,
            permissionCoordinate: coordinate
        )
        return .installed(
            NativeRuntimeCatalogInstallation(
                title: title,
                manifestAuthor: confirmation.manifestAuthor,
                dTag: dTag,
                aggregateHash: confirmation.aggregateHash,
                installedArtifact: installedArtifact
            )
        )
    }

    /// Reacquires one installed exact build from the current profile's retained
    /// verifier handle without granting or launching it.
    ///
    /// Rust owns the unfiltered installation lookup and exact-build drift
    /// checks. Native supplies only the complete installed coordinate and
    /// receives the same sealed handle shape as a fresh catalog installation.
    /// A restarted profile fails closed until artifact-owned persistent exact
    /// bytes can be reopened through a supported Rust seam.
    public func reacquireInstalledArtifact(
        _ coordinate: NativeRuntimePermissionCoordinate
    ) -> NativeRuntimeCatalogInstallResult {
        lock.lock()
        let closed = isClosed
        lock.unlock()
        guard !closed else {
            return .refused(
                NativeRuntimeCatalogFailure(
                    code: "closed",
                    detail: "The application runtime profile is closed",
                    provenance: []
                )
            )
        }
        let result = controller.reacquireInstalledArtifact(
            coordinate: RuntimeExactBuildCoordinate(
                manifestAuthor: coordinate.manifestAuthor,
                dTag: coordinate.dTag,
                aggregateHash: coordinate.aggregateHash
            )
        )
        if let failure = result.failure {
            return .refused(failure)
        }
        guard
            let confirmation = result.confirmation,
            let artifact = result.artifact,
            confirmation.manifestAuthor == coordinate.manifestAuthor,
            confirmation.dTag == coordinate.dTag,
            confirmation.aggregateHash == coordinate.aggregateHash
        else {
            return .refused(
                NativeRuntimeCatalogFailure(
                    code: "incomplete-reacquisition",
                    detail: "Rust returned no complete exact installed artifact",
                    provenance: []
                )
            )
        }
        let title = confirmation.title ?? "Untitled napplet"
        return .installed(
            NativeRuntimeCatalogInstallation(
                title: title,
                manifestAuthor: coordinate.manifestAuthor,
                dTag: coordinate.dTag,
                aggregateHash: coordinate.aggregateHash,
                installedArtifact: NativeRuntimeInstalledArtifact(
                    title: title,
                    ownerID: profileID,
                    artifact: artifact,
                    permissionCoordinate: coordinate
                )
            )
        )
    }

    /// Verifies and installs one exact named build without granting or
    /// launching it. The returned opaque handle is bound to this profile.
    public func installSignedNamed(
        title: String,
        eventJSON: Data,
        author: String,
        dTag: String,
        blobsBySHA256: [String: Data]
    ) throws -> NativeRuntimeInstalledArtifact {
        lock.lock()
        let closed = isClosed
        lock.unlock()
        guard !closed else {
            throw RuntimeNappletOpenError.installRefused(
                detail: "The application runtime profile is closed"
            )
        }

        let registration = try source.register(blobsBySHA256)
        defer { source.unregister(registration) }
        let verification = controller.verifyArtifact(
            eventJson: eventJSON,
            coordinate: .named(author: author, dTag: dTag)
        )
        guard let artifact = verification.artifact else {
            let refusal = verification.refusal
            throw RuntimeNappletOpenError.artifactRefused(
                code: refusal?.code ?? "missing-artifact",
                detail: refusal?.detail ?? "No sealed artifact was returned"
            )
        }

        controller.install(artifact: artifact)
        guard let verifiedDTag = artifact.dTag() else {
            throw RuntimeNappletOpenError.installRefused(
                detail: "The verified named artifact has no dTag"
            )
        }
        let coordinate = NativeRuntimePermissionCoordinate(
            manifestAuthor: artifact.author(),
            dTag: verifiedDTag,
            aggregateHash: artifact.aggregateHash()
        )
        let installedReview = controller.permissionReview(
            coordinate: coordinate
        )
        guard installedReview.refusal == nil,
              installedReview.review?.coordinate == coordinate
        else {
            throw RuntimeNappletOpenError.installRefused(
                detail: installedReview.refusal?.detail
                    ?? "The installed exact build was not projected by Rust"
            )
        }
        return NativeRuntimeInstalledArtifact(
            title: title,
            ownerID: profileID,
            artifact: artifact,
            permissionCoordinate: coordinate
        )
    }

    /// Launches one already-installed exact build. Permission application is a
    /// separate Rust transaction and is never performed by this operation.
    public func launchInstalled(
        _ installed: NativeRuntimeInstalledArtifact
    ) throws -> NappletArtifact {
        guard installed.ownerID == profileID else {
            throw RuntimeNappletOpenError.installedArtifactProfileMismatch
        }
        lock.lock()
        let closed = isClosed
        lock.unlock()
        guard !closed else {
            throw RuntimeNappletOpenError.launchRefused(
                detail: "The application runtime profile is closed"
            )
        }

        let artifact = installed.artifact
        let coordinate = installed.permissionCoordinate
        let priorSessions = Set(controller.snapshot().sessions.map(\.id))
        controller.launch(artifact: artifact, profile: .legacy)

        guard let launched = controller.snapshot().sessions.first(where: {
            !priorSessions.contains($0.id)
                && $0.author == coordinate.manifestAuthor
                && $0.dTag == coordinate.dTag
                && $0.aggregateHash == coordinate.aggregateHash
                && $0.state == "running"
        }) else {
            let snapshot = controller.snapshot()
            let detail = snapshot.recentErrors.last?.detail
                ?? snapshot.boundaryRefusals.last?.detail
                ?? "No new running session was created"
            throw RuntimeNappletOpenError.launchRefused(detail: detail)
        }

        let runtime = RustRuntimeNappletSession(
            profile: self,
            sessionID: launched.id,
            maximumReadBytes: Self.maximumReadBytes
        )
        lock.lock()
        if isClosed {
            lock.unlock()
            controller.stop(sessionId: launched.id)
            throw RuntimeNappletOpenError.launchRefused(
                detail: "The application runtime profile closed during launch"
            )
        }
        sessions[launched.id] = WeakSession(runtime)
        lock.unlock()

        return NappletArtifact(
            title: installed.title,
            reader: runtime,
            runtimeSession: runtime,
            negotiatedDomains: launched.domains
        )
    }

    /// Compatibility helper retained for existing Apple package callers.
    /// Product launch flows must use install, atomic permission review, and
    /// launch as separate operations.
    public func openSignedNamed(
        title: String,
        eventJSON: Data,
        author: String,
        dTag: String,
        blobsBySHA256: [String: Data],
        grantDomains: [String]
    ) throws -> NappletArtifact {
        let installed = try installSignedNamed(
            title: title,
            eventJSON: eventJSON,
            author: author,
            dTag: dTag,
            blobsBySHA256: blobsBySHA256
        )
        for domain in grantDomains {
            controller.setGrant(
                artifact: installed.artifact,
                capability: domain,
                sensitivity: .ordinary,
                decision: .allowExactBuild
            )
        }
        return try launchInstalled(installed)
    }

    public func close() {
        accountLock.lock()
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            accountLock.unlock()
            return
        }
        isClosed = true
        let observation = observation
        self.observation = nil
        let activeSessions = sessions.values.compactMap(\.value)
        sessions.removeAll()
        activityObservers.removeAll()
        libraryObservers.removeAll()
        catalogObservers.removeAll()
        pendingWriteObservers.removeAll()
        receiptObservers.removeAll()
        lock.unlock()

        observation?.stop()
        for session in activeSessions {
            session.profileDidClose()
        }
        appearanceSource.close()
        settingsExecutor.close()
        incActionExecutor.close()
        controller.close()
        accountLock.unlock()
    }

    /// Installs or removes the application-owned NAP-INC action handler.
    /// Delivery occurs on the main dispatch queue and is bounded by the
    /// native executor; removing the handler purges queued actions.
    public func setIncActionHandler(
        _ handler: NativeWorkbenchActionHandler?
    ) {
        incActionExecutor.setHandler(handler)
    }

    public func accountSnapshot() -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.accountSnapshot()
        accountLock.unlock()
        return update
    }

    public func registerLocalAccount(
        secretKey: String
    ) -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.registerLocalAccount(secretKey: secretKey)
        if
            update.accepted,
            let handle = update.handle,
            let accountVault
        {
            do {
                try accountVault.upsertLocalSigner(
                    publicKey: handle.publicKey,
                    secret: secretKey,
                    maximumAccounts: Self.maximumAccounts
                )
            } catch {
                accountPersistenceProblem =
                    accountPersistenceProblem ?? .registerFailed
            }
        }
        accountLock.unlock()
        return update
    }

    public func registerReadOnlyAccount(
        publicIdentity: String
    ) -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.registerReadOnlyAccount(
            publicIdentity: publicIdentity
        )
        if
            update.accepted,
            let handle = update.handle,
            let accountVault
        {
            do {
                try accountVault.upsertReadOnly(
                    publicKey: handle.publicKey,
                    maximumAccounts: Self.maximumAccounts
                )
            } catch {
                accountPersistenceProblem =
                    accountPersistenceProblem ?? .registerFailed
            }
        }
        accountLock.unlock()
        return update
    }

    public func activateLocalAccount(
        handle: NativeRuntimeAccountHandle
    ) -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.activateLocalAccount(handle: handle)
        if update.accepted, let accountVault {
            do {
                try accountVault.setActive(
                    publicKey: update.snapshot?.activePublicKey,
                    maximumAccounts: Self.maximumAccounts
                )
            } catch {
                accountPersistenceProblem =
                    accountPersistenceProblem ?? .activationFailed
            }
        }
        accountLock.unlock()
        return update
    }

    public func logoutLocalAccount() -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.logoutLocalAccount()
        if update.accepted, let accountVault {
            do {
                try accountVault.setActive(
                    publicKey: nil,
                    maximumAccounts: Self.maximumAccounts
                )
            } catch {
                accountPersistenceProblem =
                    accountPersistenceProblem ?? .logoutFailed
            }
        }
        accountLock.unlock()
        return update
    }

    public func removeLocalAccount(
        handle: NativeRuntimeAccountHandle
    ) -> NativeRuntimeAccountUpdate {
        accountLock.lock()
        let update = controller.removeLocalAccount(handle: handle)
        if update.accepted, let accountVault {
            do {
                try accountVault.remove(
                    publicKey: handle.publicKey,
                    maximumAccounts: Self.maximumAccounts
                )
            } catch {
                accountPersistenceProblem =
                    accountPersistenceProblem ?? .removalFailed
            }
        }
        accountLock.unlock()
        return update
    }

    public func accountPersistenceIssue()
        -> NativeRuntimeAccountPersistenceIssue?
    {
        accountLock.lock()
        let issue = accountPersistenceProblem
        accountLock.unlock()
        return issue
    }

    private func restorePersistedAccounts() {
        guard let accountVault else {
            return
        }

        accountLock.lock()
        defer { accountLock.unlock() }
        let stored: NativeAccountVaultSnapshot
        do {
            stored = try accountVault.load(
                maximumAccounts: Self.maximumAccounts
            )
        } catch {
            accountPersistenceProblem = .restoreFailed
            return
        }

        var restoredHandles: [
            String: NativeRuntimeAccountHandle
        ] = [:]
        restoredHandles.reserveCapacity(stored.accounts.count)
        var restoreFailed = false
        for account in stored.accounts {
            let update: NativeRuntimeAccountUpdate
            switch account.material {
            case let .localSigner(secret):
                update = controller.registerLocalAccount(
                    secretKey: secret
                )
            case .readOnly:
                update = controller.registerReadOnlyAccount(
                    publicIdentity: account.publicKey
                )
            }
            guard
                update.accepted,
                let handle = update.handle,
                handle.publicKey == account.publicKey
            else {
                if let unexpectedHandle = update.handle {
                    _ = controller.removeLocalAccount(
                        handle: unexpectedHandle
                    )
                }
                restoreFailed = true
                continue
            }
            restoredHandles[account.publicKey] = handle
        }

        if let activePublicKey = stored.activePublicKey {
            guard let activeHandle = restoredHandles[activePublicKey] else {
                accountPersistenceProblem = .restoreFailed
                let revision = controller.snapshot().revision
                lastActivityRevision = revision
                lastLibraryRevision = revision
                return
            }
            let activation = controller.activateLocalAccount(
                handle: activeHandle
            )
            if
                !activation.accepted
                    || activation.snapshot?.activePublicKey != activePublicKey
            {
                restoreFailed = true
            }
        }
        accountPersistenceProblem = restoreFailed ? .restoreFailed : nil
        let revision = controller.snapshot().revision
        lastActivityRevision = revision
        lastLibraryRevision = revision
    }

    public func saveWorkspace(
        _ workspace: NativeRuntimeWorkspaceDefinition
    ) -> NativeRuntimeWorkspaceUpdate {
        controller.saveWorkspace(workspace: workspace)
    }

    public func restoreWorkspaces() -> NativeRuntimeWorkspaceRestore {
        controller.restoreWorkspaces()
    }

    /// Returns one bounded Rust-owned review for an installed exact build.
    /// This operation never grants or launches the napplet.
    public func permissionReview(
        for coordinate: NativeRuntimePermissionCoordinate
    ) -> NativeRuntimePermissionReviewResult {
        controller.permissionReview(coordinate: coordinate)
    }

    /// Applies one complete exact-build decision set atomically in Rust.
    /// Success never launches the napplet; launch remains a separate operation.
    public func applyPermissionDecisions(
        _ batch: NativeRuntimePermissionDecisionBatch
    ) -> NativeRuntimePermissionBatchUpdate {
        controller.applyPermissionDecisions(batch: batch)
    }

    /// Resolves one Rust-retained provider write proposal. The native shell
    /// supplies only the opaque operation id and decision; the frozen write
    /// remains owned by RuntimeApp.
    public func decideProviderWrite(
        operationID: UInt64,
        approve: Bool
    ) {
        controller.decideProviderWrite(
            operationId: operationID,
            approve: approve
        )
    }

    /// Returns the latest bounded set of Rust-retained provider writes.
    public func pendingWriteProjection()
        -> NativeRuntimePendingWriteProjection
    {
        NativeRuntimePendingWriteProjection(controller.snapshot())
    }

    /// Observes the profile-owned pending-write replacement stream. The
    /// callback receives an authoritative replacement synchronously, followed
    /// by conflated latest updates from the permanent Rust observation.
    public func observePendingWrites(
        _ receive: @escaping @Sendable (NativeRuntimePendingWriteUpdate) -> Void
    ) throws -> NativeRuntimePendingWriteObservation {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            throw NativeRuntimePendingWriteObservationError.profileClosed
        }
        guard pendingWriteObservers.count
            < Self.maximumApplicationPendingWriteObservers
        else {
            lock.unlock()
            throw NativeRuntimePendingWriteObservationError.observerCapacity(
                maximum: Self.maximumApplicationPendingWriteObservers
            )
        }
        let identifier = UUID()
        let authoritative = NativeRuntimePendingWriteProjection(
            controller.snapshot()
        )
        pendingWriteObservers[identifier] = PendingWriteObserverEntry(
            receive: receive,
            lastDeliveredRevision: authoritative.revision
        )
        lock.unlock()

        let observation = NativeRuntimePendingWriteObservation { [weak self] in
            self?.removePendingWriteObserver(identifier)
        }
        receive(.authoritative(authoritative))
        drainPendingWriteUpdates(for: identifier)
        return observation
    }

    /// Observes the bounded durable receipt replacement owned by the profile.
    /// Delivery state is presented mechanically; native does not infer an
    /// outcome from relay payloads.
    public func observeReceipts(
        _ receive: @escaping @Sendable (NativeRuntimeReceiptUpdate) -> Void
    ) throws -> NativeRuntimeReceiptObservation {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            throw NativeRuntimeReceiptObservationError.profileClosed
        }
        guard receiptObservers.count < Self.maximumApplicationReceiptObservers
        else {
            lock.unlock()
            throw NativeRuntimeReceiptObservationError.observerCapacity(
                maximum: Self.maximumApplicationReceiptObservers
            )
        }
        let identifier = UUID()
        let authoritative = NativeRuntimeReceiptProjection(controller.snapshot())
        receiptObservers[identifier] = ReceiptObserverEntry(
            receive: receive,
            lastDeliveredRevision: authoritative.revision
        )
        lock.unlock()

        let observation = NativeRuntimeReceiptObservation { [weak self] in
            self?.removeReceiptObserver(identifier)
        }
        receive(.authoritative(authoritative))
        drainReceiptUpdates(for: identifier)
        return observation
    }

    /// Returns the latest complete installed-library replacement from the
    /// Rust-owned profile snapshot.
    public func installedLibraryProjection()
        -> NativeRuntimeLibraryProjection
    {
        NativeRuntimeLibraryProjection(controller.snapshot())
    }

    /// Applies the Rust-owned finite installed-library filter.
    public func setInstalledLibraryFilter(_ query: String) {
        controller.setLibraryFilter(query: query)
    }

    public func suspendInstalledSession(_ sessionID: UInt64) {
        controller.suspend(sessionId: sessionID)
    }

    public func resumeInstalledSession(_ sessionID: UInt64) {
        controller.resume(sessionId: sessionID)
    }

    public func assignInstalledBuild(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        toWorkspaceID workspaceID: String
    ) {
        controller.assignBuildToWorkspace(
            workspaceId: workspaceID,
            coordinate: runtimeCoordinate(exactBuild)
        )
    }

    public func clearInstalledBuildAssignment(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        fromWorkspaceID workspaceID: String
    ) {
        controller.clearBuildFromWorkspace(
            workspaceId: workspaceID,
            coordinate: runtimeCoordinate(exactBuild)
        )
    }

    public func uninstallInstalledBuild(
        _ exactBuild: NativeRuntimeLibraryExactBuild
    ) {
        controller.uninstallBuild(coordinate: runtimeCoordinate(exactBuild))
    }

    /// Returns the latest complete, bounded runtime activity replacement.
    public func activityProjection(
        for scope: NativeRuntimeActivityScope
    ) -> NativeRuntimeActivityProjection {
        NativeRuntimeActivityProjection(controller.snapshot(), scope: scope)
    }

    /// Adds one bounded application observer to the profile's permanent NMP
    /// catalog feed. Registration synchronously delivers the latest complete
    /// replacement; subsequent updates are conflated to one pending latest
    /// value while that authoritative callback is in flight.
    public func observeCatalog(
        _ receive: @escaping @Sendable (NativeRuntimeCatalogUpdate) -> Void
    ) throws -> NativeRuntimeCatalogObservation {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            throw NativeRuntimeCatalogObservationError.profileClosed
        }
        guard catalogObservers.count
            < Self.maximumApplicationCatalogObservers
        else {
            lock.unlock()
            throw NativeRuntimeCatalogObservationError.observerCapacity(
                maximum: Self.maximumApplicationCatalogObservers
            )
        }
        let identifier = UUID()
        let authoritative = lastCatalogSnapshot
        catalogObservers[identifier] = CatalogObserverEntry(
            receive: receive,
            lastDeliveredRevision: authoritative.revision
        )
        lock.unlock()

        let observation = NativeRuntimeCatalogObservation { [weak self] in
            self?.removeCatalogObserver(identifier)
        }
        receive(.authoritative(authoritative))
        drainPendingCatalogUpdates(for: identifier)
        return observation
    }

    /// Adds one bounded application observer to the profile's single Rust
    /// observation stream. Admission refusal is explicit, and the receiver is
    /// called synchronously with an authoritative replacement before return.
    public func observeActivity(
        scope: NativeRuntimeActivityScope,
        _ receive: @escaping @Sendable (NativeRuntimeActivityUpdate) -> Void
    ) throws -> NativeRuntimeActivityObservation {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            throw NativeRuntimeActivityObservationError.profileClosed
        }
        guard activityObservers.count
            < Self.maximumApplicationActivityObservers
        else {
            lock.unlock()
            throw NativeRuntimeActivityObservationError.observerCapacity(
                maximum: Self.maximumApplicationActivityObservers
            )
        }
        let identifier = UUID()
        activityObservers[identifier] = ActivityObserverEntry(
            scope: scope,
            receive: receive
        )
        lock.unlock()

        let observation = NativeRuntimeActivityObservation {
            [weak self] in
            self?.removeActivityObserver(identifier)
        }
        receive(
            .authoritative(
                NativeRuntimeActivityProjection(
                    controller.snapshot(),
                    scope: scope
                )
            )
        )
        return observation
    }

    /// Adds one bounded application observer to the installed-library view on
    /// the profile's existing Rust observation stream.
    public func observeInstalledLibrary(
        _ receive: @escaping @Sendable (NativeRuntimeLibraryUpdate) -> Void
    ) throws -> NativeRuntimeLibraryObservation {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            throw NativeRuntimeLibraryObservationError.profileClosed
        }
        guard libraryObservers.count
            < Self.maximumApplicationLibraryObservers
        else {
            lock.unlock()
            throw NativeRuntimeLibraryObservationError.observerCapacity(
                maximum: Self.maximumApplicationLibraryObservers
            )
        }
        let identifier = UUID()
        let authoritative = NativeRuntimeLibraryProjection(
            controller.snapshot()
        )
        libraryObservers[identifier] = LibraryObserverEntry(
            receive: receive,
            lastDeliveredRevision: authoritative.revision
        )
        lock.unlock()

        let observation = NativeRuntimeLibraryObservation { [weak self] in
            self?.removeLibraryObserver(identifier)
        }
        receive(.authoritative(authoritative))
        drainPendingLibraryUpdates(for: identifier)
        return observation
    }

    public func update(frame: RuntimeObservationFrame) {
        lock.lock()
        if isClosed {
            lock.unlock()
            return
        }
        sessions = sessions.filter { $0.value.value != nil }
        let activeSessions = sessions.values.compactMap(\.value)
        let previousActivityRevision = lastActivityRevision
        let previousLibraryRevision = lastLibraryRevision
        let previousCatalogRevision = lastCatalogSnapshot.revision
        let previousPendingWriteRevision = lastPendingWriteRevision
        let previousReceiptRevision = lastReceiptRevision
        lastActivityRevision = frame.snapshot.revision
        lastLibraryRevision = frame.snapshot.revision
        lastPendingWriteRevision = frame.snapshot.revision
        lastReceiptRevision = frame.snapshot.revision
        if frame.catalog.revision >= previousCatalogRevision {
            lastCatalogSnapshot = frame.catalog
        }
        let activityObservers = Array(activityObservers.values)
        var libraryDeliveries: [
            (receive: LibraryReceiver, update: NativeRuntimeLibraryUpdate)
        ] = []
        var catalogDeliveries: [
            (receive: CatalogReceiver, update: NativeRuntimeCatalogUpdate)
        ] = []
        var pendingWriteDeliveries: [
            (receive: PendingWriteReceiver, update: NativeRuntimePendingWriteUpdate)
        ] = []
        var receiptDeliveries: [
            (receive: ReceiptReceiver, update: NativeRuntimeReceiptUpdate)
        ] = []
        if frame.snapshot.revision > previousLibraryRevision
            || frame.eventCursorWasStale
        {
            let projection = NativeRuntimeLibraryProjection(frame.snapshot)
            let update = NativeRuntimeLibraryUpdate.next(
                projection,
                predecessorRevision: previousLibraryRevision,
                eventCursorWasStale: frame.eventCursorWasStale
            )
            for identifier in Array(libraryObservers.keys) {
                guard var observer = libraryObservers[identifier] else {
                    continue
                }
                let isNewer = projection.revision
                    > observer.lastDeliveredRevision
                let isCurrentStaleReplacement = frame.eventCursorWasStale
                    && projection.revision
                        == observer.lastDeliveredRevision
                guard isNewer || isCurrentStaleReplacement else {
                    continue
                }
                if observer.isReadyForNext {
                    observer.lastDeliveredRevision = projection.revision
                    libraryObservers[identifier] = observer
                    libraryDeliveries.append((observer.receive, update))
                    continue
                }
                if let pendingUpdate = observer.pendingUpdate,
                   projection.revision < libraryRevision(of: pendingUpdate)
                {
                    continue
                }
                observer.pendingUpdate = update
                libraryObservers[identifier] = observer
            }
        }
        if frame.catalog.revision > previousCatalogRevision {
            let update = NativeRuntimeCatalogUpdate.next(
                frame.catalog,
                predecessorRevision: previousCatalogRevision
            )
            for identifier in Array(catalogObservers.keys) {
                guard var observer = catalogObservers[identifier],
                      frame.catalog.revision
                          > observer.lastDeliveredRevision
                else {
                    continue
                }
                if observer.isReadyForNext {
                    observer.lastDeliveredRevision = frame.catalog.revision
                    catalogObservers[identifier] = observer
                    catalogDeliveries.append((observer.receive, update))
                    continue
                }
                if let pending = observer.pendingUpdate,
                   catalogRevision(of: pending) > frame.catalog.revision
                {
                    continue
                }
                observer.pendingUpdate = update
                catalogObservers[identifier] = observer
            }
        }
        if frame.snapshot.revision > previousPendingWriteRevision
            || frame.eventCursorWasStale
        {
            let projection = NativeRuntimePendingWriteProjection(frame.snapshot)
            let update = NativeRuntimePendingWriteUpdate.next(
                projection,
                predecessorRevision: previousPendingWriteRevision,
                eventCursorWasStale: frame.eventCursorWasStale
            )
            for identifier in Array(pendingWriteObservers.keys) {
                guard var observer = pendingWriteObservers[identifier] else {
                    continue
                }
                let isNewer = projection.revision
                    > observer.lastDeliveredRevision
                let isCurrentStaleReplacement = frame.eventCursorWasStale
                    && projection.revision
                        == observer.lastDeliveredRevision
                guard isNewer || isCurrentStaleReplacement else {
                    continue
                }
                if observer.isReadyForNext {
                    observer.lastDeliveredRevision = projection.revision
                    pendingWriteObservers[identifier] = observer
                    pendingWriteDeliveries.append((observer.receive, update))
                    continue
                }
                if let pendingUpdate = observer.pendingUpdate,
                   projection.revision
                        < pendingWriteRevision(of: pendingUpdate)
                {
                    continue
                }
                observer.pendingUpdate = update
                pendingWriteObservers[identifier] = observer
            }
        }
        if frame.snapshot.revision > previousReceiptRevision
            || frame.eventCursorWasStale
        {
            let projection = NativeRuntimeReceiptProjection(frame.snapshot)
            let update = NativeRuntimeReceiptUpdate.next(
                projection,
                predecessorRevision: previousReceiptRevision,
                eventCursorWasStale: frame.eventCursorWasStale
            )
            for identifier in Array(receiptObservers.keys) {
                guard var observer = receiptObservers[identifier] else {
                    continue
                }
                let isNewer = projection.revision > observer.lastDeliveredRevision
                let isCurrentStaleReplacement = frame.eventCursorWasStale
                    && projection.revision == observer.lastDeliveredRevision
                guard isNewer || isCurrentStaleReplacement else { continue }
                if observer.isReadyForNext {
                    observer.lastDeliveredRevision = projection.revision
                    receiptObservers[identifier] = observer
                    receiptDeliveries.append((observer.receive, update))
                    continue
                }
                if let pendingUpdate = observer.pendingUpdate,
                   projection.revision < receiptRevision(of: pendingUpdate)
                {
                    continue
                }
                observer.pendingUpdate = update
                receiptObservers[identifier] = observer
            }
        }
        lock.unlock()
        settingsExecutor.retainRunningSessions(
            Set(frame.snapshot.sessions.filter { $0.state == "running" }.map(\.id))
        )
        for session in activeSessions {
            session.deliver(frame: frame)
        }
        if frame.snapshot.revision > previousActivityRevision
            || frame.eventCursorWasStale
        {
            for observer in activityObservers {
                observer.receive(
                    .next(
                        NativeRuntimeActivityProjection(
                            frame.snapshot,
                            scope: observer.scope
                        ),
                        predecessorRevision: previousActivityRevision,
                        eventCursorWasStale: frame.eventCursorWasStale
                    )
                )
            }
        }
        for delivery in libraryDeliveries {
            delivery.receive(delivery.update)
        }
        for delivery in catalogDeliveries {
            delivery.receive(delivery.update)
        }
        for delivery in pendingWriteDeliveries {
            delivery.receive(delivery.update)
        }
        for delivery in receiptDeliveries {
            delivery.receive(delivery.update)
        }
    }

    private func removeActivityObserver(_ identifier: UUID) {
        lock.lock()
        activityObservers.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func removeLibraryObserver(_ identifier: UUID) {
        lock.lock()
        libraryObservers.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func removeCatalogObserver(_ identifier: UUID) {
        lock.lock()
        catalogObservers.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func removePendingWriteObserver(_ identifier: UUID) {
        lock.lock()
        pendingWriteObservers.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func removeReceiptObserver(_ identifier: UUID) {
        lock.lock()
        receiptObservers.removeValue(forKey: identifier)
        lock.unlock()
    }

    private func drainPendingLibraryUpdates(for identifier: UUID) {
        while true {
            lock.lock()
            guard !isClosed, var observer = libraryObservers[identifier] else {
                lock.unlock()
                return
            }
            guard let pendingUpdate = observer.pendingUpdate else {
                observer.isReadyForNext = true
                libraryObservers[identifier] = observer
                lock.unlock()
                return
            }
            observer.pendingUpdate = nil
            observer.lastDeliveredRevision = max(
                observer.lastDeliveredRevision,
                libraryRevision(of: pendingUpdate)
            )
            libraryObservers[identifier] = observer
            let receive = observer.receive
            lock.unlock()
            receive(pendingUpdate)
        }
    }

    private func drainPendingCatalogUpdates(for identifier: UUID) {
        while true {
            lock.lock()
            guard !isClosed, var observer = catalogObservers[identifier] else {
                lock.unlock()
                return
            }
            guard let pendingUpdate = observer.pendingUpdate else {
                observer.isReadyForNext = true
                catalogObservers[identifier] = observer
                lock.unlock()
                return
            }
            observer.pendingUpdate = nil
            observer.lastDeliveredRevision = max(
                observer.lastDeliveredRevision,
                catalogRevision(of: pendingUpdate)
            )
            catalogObservers[identifier] = observer
            let receive = observer.receive
            lock.unlock()
            receive(pendingUpdate)
        }
    }

    private func drainPendingWriteUpdates(for identifier: UUID) {
        while true {
            lock.lock()
            guard !isClosed, var observer = pendingWriteObservers[identifier]
            else {
                lock.unlock()
                return
            }
            guard let pendingUpdate = observer.pendingUpdate else {
                observer.isReadyForNext = true
                pendingWriteObservers[identifier] = observer
                lock.unlock()
                return
            }
            observer.pendingUpdate = nil
            observer.lastDeliveredRevision = max(
                observer.lastDeliveredRevision,
                pendingWriteRevision(of: pendingUpdate)
            )
            pendingWriteObservers[identifier] = observer
            let receive = observer.receive
            lock.unlock()
            receive(pendingUpdate)
        }
    }

    private func drainReceiptUpdates(for identifier: UUID) {
        while true {
            lock.lock()
            guard !isClosed, var observer = receiptObservers[identifier]
            else {
                lock.unlock()
                return
            }
            guard let pendingUpdate = observer.pendingUpdate else {
                observer.isReadyForNext = true
                receiptObservers[identifier] = observer
                lock.unlock()
                return
            }
            observer.pendingUpdate = nil
            observer.lastDeliveredRevision = max(
                observer.lastDeliveredRevision,
                receiptRevision(of: pendingUpdate)
            )
            receiptObservers[identifier] = observer
            let receive = observer.receive
            lock.unlock()
            receive(pendingUpdate)
        }
    }

    private func libraryRevision(
        of update: NativeRuntimeLibraryUpdate
    ) -> UInt64 {
        switch update {
        case let .authoritative(projection),
             let .next(projection, _, _):
            projection.revision
        }
    }

    private func catalogRevision(
        of update: NativeRuntimeCatalogUpdate
    ) -> UInt64 {
        switch update {
        case let .authoritative(snapshot),
             let .next(snapshot, _):
            snapshot.revision
        }
    }

    private func pendingWriteRevision(
        of update: NativeRuntimePendingWriteUpdate
    ) -> UInt64 {
        switch update {
        case let .authoritative(projection),
             let .next(projection, _, _):
            projection.revision
        }
    }

    private func receiptRevision(
        of update: NativeRuntimeReceiptUpdate
    ) -> UInt64 {
        switch update {
        case let .authoritative(projection),
             let .next(projection, _, _):
            projection.revision
        }
    }

    private func runtimeCoordinate(
        _ exactBuild: NativeRuntimeLibraryExactBuild
    ) -> RuntimeExactBuildCoordinate {
        RuntimeExactBuildCoordinate(
            manifestAuthor: exactBuild.manifestAuthor,
            dTag: exactBuild.dTag,
            aggregateHash: exactBuild.aggregateHash
        )
    }

    fileprivate func readVerified(
        sessionID: UInt64,
        logicalPath: String,
        maximumBytes: UInt64
    ) -> VerifiedRead {
        controller.readVerified(
            sessionId: sessionID,
            logicalPath: logicalPath,
            maximumBytes: maximumBytes
        )
    }

    fileprivate func mappedEnvelope(sessionID: UInt64, bytes: Data) {
        controller.mappedEnvelope(sessionId: sessionID, bytes: bytes)
    }

    fileprivate func stopSession(_ sessionID: UInt64) {
        lock.lock()
        let shouldStop = !isClosed && sessions.removeValue(forKey: sessionID) != nil
        lock.unlock()
        if shouldStop {
            controller.stop(sessionId: sessionID)
        }
    }

    fileprivate func crashSession(_ sessionID: UInt64, reason: String) {
        lock.lock()
        let shouldCrash = !isClosed && sessions[sessionID]?.value != nil
        lock.unlock()
        if shouldCrash {
            controller.crash(sessionId: sessionID, reason: reason)
        }
    }

    var snapshotForTesting: RuntimeSnapshot {
        controller.snapshot()
    }

    var catalogSnapshotForTesting: RuntimeCatalogFeedSnapshot {
        controller.catalogFeedSnapshot()
    }

    deinit {
        close()
    }
}

protocol TrustedNappletRuntimeSession: VerifiedArtifactByteReader {
    var sessionID: UInt64 { get }

    func setResponseSink(_ sink: (@Sendable (Data) -> Void)?)
    func mappedEnvelope(_ bytes: Data)
    func stop()
    func crash(reason: String)
}

/// A sealed, exact-build session. The generated controller is the only owner
/// of identity, grants, lifecycle, provider routing, and artifact reads.
final class RustRuntimeNappletSession: TrustedNappletRuntimeSession, @unchecked Sendable {
    let sessionID: UInt64

    private weak var profile: NativeRuntimeProfile?
    private let maximumReadBytes: UInt64
    private let lock = NSLock()
    private var responseSink: (@Sendable (Data) -> Void)?
    private var isStopped = false

    init(
        profile: NativeRuntimeProfile,
        sessionID: UInt64,
        maximumReadBytes: UInt64
    ) {
        self.profile = profile
        self.sessionID = sessionID
        self.maximumReadBytes = maximumReadBytes
    }

    func readSealed(logicalPath: String) throws -> SealedArtifactBytes? {
        guard let profile else { return nil }
        switch profile.readVerified(
            sessionID: sessionID,
            logicalPath: logicalPath,
            maximumBytes: maximumReadBytes
        ) {
        case let .bytes(bytes, _, sha256):
            return SealedArtifactBytes(
                logicalPath: logicalPath,
                sha256: sha256,
                bytes: bytes
            )
        case .refused:
            return nil
        }
    }

    func setResponseSink(_ sink: (@Sendable (Data) -> Void)?) {
        lock.lock()
        responseSink = sink
        lock.unlock()
    }

    func mappedEnvelope(_ bytes: Data) {
        lock.lock()
        let stopped = isStopped
        lock.unlock()
        guard !stopped else { return }
        profile?.mappedEnvelope(sessionID: sessionID, bytes: bytes)
    }

    fileprivate func deliver(frame: RuntimeObservationFrame) {
        lock.lock()
        let sink = responseSink
        let stopped = isStopped
        lock.unlock()
        guard !stopped, let sink else { return }

        for event in frame.events
        where (event.kind == "envelope-handled"
            || event.kind == "provider-push")
            && event.sessionId == sessionID {
            guard let response = event.responseJson,
                  let bytes = response.data(using: .utf8)
            else {
                continue
            }
            sink(bytes)
        }
    }

    func stop() {
        lock.lock()
        guard !isStopped else {
            lock.unlock()
            return
        }
        isStopped = true
        responseSink = nil
        let profile = profile
        self.profile = nil
        lock.unlock()

        profile?.stopSession(sessionID)
    }

    func crash(reason: String) {
        lock.lock()
        let stopped = isStopped
        lock.unlock()
        guard !stopped else { return }
        profile?.crashSession(sessionID, reason: reason)
    }

    fileprivate func profileDidClose() {
        lock.lock()
        isStopped = true
        responseSink = nil
        profile = nil
        lock.unlock()
    }

    deinit {
        stop()
    }
}
