import Foundation
import NMPNativeRuntime

public enum RuntimeNappletOpenError: Error, LocalizedError, Equatable {
    case invalidStorageRoot
    case artifactSourceRefused(detail: String)
    case artifactRefused(code: String, detail: String)
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
            "Saved local accounts could not be restored securely."
        case .registerFailed:
            "The local account is available for this session but was not saved securely."
        case .activationFailed:
            "The active account changed for this session but was not saved securely."
        case .logoutFailed:
            "The account is logged out for this session but secure persistence was not updated."
        case .removalFailed:
            "The local account was removed for this session but secure persistence was not fully updated."
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

    public init(
        storageRoot: URL,
        indexerRelays: [String] = [],
        appRelays: [String] = [],
        fallbackRelays: [String] = [],
        allowedLocalRelayHosts: [String] = [],
        accountPersistence: NativeRuntimeAccountPersistence = .transient
    ) {
        self.storageRoot = storageRoot
        self.indexerRelays = indexerRelays
        self.appRelays = appRelays
        self.fallbackRelays = fallbackRelays
        self.allowedLocalRelayHosts = allowedLocalRelayHosts
        self.accountPersistence = accountPersistence
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

    private struct ActivityObserverEntry {
        let scope: NativeRuntimeActivityScope
        let receive: ActivityReceiver
    }

    private final class WeakSession {
        weak var value: RustRuntimeNappletSession?

        init(_ value: RustRuntimeNappletSession) {
            self.value = value
        }
    }

    private static let maximumReadBytes: UInt64 = 8 * 1_024 * 1_024
    private static let maximumApplicationActivityObservers = 8
    private static let maximumLocalAccounts = 32

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
    private var lastActivityRevision: UInt64
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
                maximumBlobSources: 8
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
        lastActivityRevision = controller.snapshot().revision
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

    public func openSignedNamed(
        title: String,
        eventJSON: Data,
        author: String,
        dTag: String,
        blobsBySHA256: [String: Data],
        grantDomains: [String]
    ) throws -> NappletArtifact {
        lock.lock()
        let closed = isClosed
        lock.unlock()
        guard !closed else {
            throw RuntimeNappletOpenError.launchRefused(
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
        for domain in grantDomains {
            controller.setGrant(
                artifact: artifact,
                capability: domain,
                sensitivity: .ordinary,
                decision: .allowExactBuild
            )
        }
        let priorSessions = Set(controller.snapshot().sessions.map(\.id))
        controller.launch(artifact: artifact, profile: .legacy)

        let aggregateHash = artifact.aggregateHash()
        guard let launched = controller.snapshot().sessions.first(where: {
            !priorSessions.contains($0.id)
                && $0.author == author
                && $0.dTag == dTag
                && $0.aggregateHash == aggregateHash
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
            title: title,
            reader: runtime,
            runtimeSession: runtime,
            negotiatedDomains: launched.domains
        )
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
                try accountVault.upsert(
                    publicKey: handle.publicKey,
                    secret: secretKey,
                    maximumAccounts: Self.maximumLocalAccounts
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
                    maximumAccounts: Self.maximumLocalAccounts
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
                    maximumAccounts: Self.maximumLocalAccounts
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
                    maximumAccounts: Self.maximumLocalAccounts
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
                maximumAccounts: Self.maximumLocalAccounts
            )
        } catch {
            accountPersistenceProblem = .restoreFailed
            return
        }

        var restoredHandles: [
            String: NativeRuntimeAccountHandle
        ] = [:]
        restoredHandles.reserveCapacity(stored.credentials.count)
        var restoreFailed = false
        for credential in stored.credentials {
            let update = controller.registerLocalAccount(
                secretKey: credential.secret
            )
            guard
                update.accepted,
                let handle = update.handle,
                handle.publicKey == credential.publicKey
            else {
                if let unexpectedHandle = update.handle {
                    _ = controller.removeLocalAccount(
                        handle: unexpectedHandle
                    )
                }
                restoreFailed = true
                continue
            }
            restoredHandles[credential.publicKey] = handle
        }

        if let activePublicKey = stored.activePublicKey {
            guard let activeHandle = restoredHandles[activePublicKey] else {
                accountPersistenceProblem = .restoreFailed
                lastActivityRevision = controller.snapshot().revision
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
        lastActivityRevision = controller.snapshot().revision
    }

    public func saveWorkspace(
        _ workspace: NativeRuntimeWorkspaceDefinition
    ) -> NativeRuntimeWorkspaceUpdate {
        controller.saveWorkspace(workspace: workspace)
    }

    public func restoreWorkspaces() -> NativeRuntimeWorkspaceRestore {
        controller.restoreWorkspaces()
    }

    /// Returns the latest complete, bounded runtime activity replacement.
    public func activityProjection(
        for scope: NativeRuntimeActivityScope
    ) -> NativeRuntimeActivityProjection {
        NativeRuntimeActivityProjection(controller.snapshot(), scope: scope)
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

    public func update(frame: RuntimeObservationFrame) {
        lock.lock()
        if isClosed {
            lock.unlock()
            return
        }
        sessions = sessions.filter { $0.value.value != nil }
        let activeSessions = sessions.values.compactMap(\.value)
        let previousRevision = lastActivityRevision
        lastActivityRevision = frame.snapshot.revision
        let activityObservers = Array(activityObservers.values)
        lock.unlock()
        settingsExecutor.retainRunningSessions(
            Set(frame.snapshot.sessions.filter { $0.state == "running" }.map(\.id))
        )
        for session in activeSessions {
            session.deliver(frame: frame)
        }
        guard frame.snapshot.revision > previousRevision
                || frame.eventCursorWasStale
        else {
            return
        }
        for observer in activityObservers {
            observer.receive(
                .next(
                    NativeRuntimeActivityProjection(
                        frame.snapshot,
                        scope: observer.scope
                    ),
                    predecessorRevision: previousRevision,
                    eventCursorWasStale: frame.eventCursorWasStale
                )
            )
        }
    }

    private func removeActivityObserver(_ identifier: UUID) {
        lock.lock()
        activityObservers.removeValue(forKey: identifier)
        lock.unlock()
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
