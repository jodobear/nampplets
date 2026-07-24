import Foundation
import NMPNativeRuntimeApple

public enum RuntimeWorkbenchLibraryAdmissionRefusal:
    Error,
    LocalizedError,
    Equatable
{
    case subscriberCapacity(maximum: Int)

    public var errorDescription: String? {
        switch self {
        case .subscriberCapacity(let maximum):
            "The Workbench library subscriber limit of \(maximum) was reached."
        }
    }
}

private enum RuntimeWorkbenchLibraryProjectionError:
    Error,
    LocalizedError
{
    case invalidExactBuild
    case invalidBuild
    case invalidWorkspace
    case invalidRefusal
    case invalidSnapshot

    var errorDescription: String? {
        switch self {
        case .invalidExactBuild:
            "The native projection contained an invalid exact-build identity."
        case .invalidBuild:
            "The native projection contained an invalid installed-build row."
        case .invalidWorkspace:
            "The native projection contained an invalid workspace row."
        case .invalidRefusal:
            "The native projection contained an invalid refusal."
        case .invalidSnapshot:
            "The native projection exceeded the Workbench snapshot contract."
        }
    }
}

protocol RuntimeWorkbenchNativeLibraryObservation:
    AnyObject,
    Sendable
{
    func cancel()
}

extension NativeRuntimeLibraryObservation:
    RuntimeWorkbenchNativeLibraryObservation
{}

protocol RuntimeWorkbenchNativeLibraryService:
    AnyObject,
    Sendable
{
    func projection() -> NativeRuntimeLibraryProjection

    func observe(
        _ receive: @escaping @Sendable (NativeRuntimeLibraryUpdate) -> Void
    ) throws -> any RuntimeWorkbenchNativeLibraryObservation

    func setFilter(_ query: String)
    func suspend(sessionID: UInt64)
    func resume(sessionID: UInt64)
    func assign(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        toWorkspaceID workspaceID: String
    )
    func clearAssignment(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        fromWorkspaceID workspaceID: String
    )
    func uninstall(_ exactBuild: NativeRuntimeLibraryExactBuild)
}

private final class ProfileNativeLibraryService:
    RuntimeWorkbenchNativeLibraryService,
    @unchecked Sendable
{
    private let profile: WorkbenchRuntimeProfile

    init(profile: WorkbenchRuntimeProfile) {
        self.profile = profile
    }

    func projection() -> NativeRuntimeLibraryProjection {
        profile.native.installedLibraryProjection()
    }

    func observe(
        _ receive: @escaping @Sendable (NativeRuntimeLibraryUpdate) -> Void
    ) throws -> any RuntimeWorkbenchNativeLibraryObservation {
        try profile.native.observeInstalledLibrary(receive)
    }

    func setFilter(_ query: String) {
        profile.native.setInstalledLibraryFilter(query)
    }

    func suspend(sessionID: UInt64) {
        profile.native.suspendInstalledSession(sessionID)
    }

    func resume(sessionID: UInt64) {
        profile.native.resumeInstalledSession(sessionID)
    }

    func assign(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        toWorkspaceID workspaceID: String
    ) {
        profile.native.assignInstalledBuild(
            exactBuild,
            toWorkspaceID: workspaceID
        )
    }

    func clearAssignment(
        _ exactBuild: NativeRuntimeLibraryExactBuild,
        fromWorkspaceID workspaceID: String
    ) {
        profile.native.clearInstalledBuildAssignment(
            exactBuild,
            fromWorkspaceID: workspaceID
        )
    }

    func uninstall(_ exactBuild: NativeRuntimeLibraryExactBuild) {
        profile.native.uninstallInstalledBuild(exactBuild)
    }
}

/// Real Workbench adapter over the profile's single pushed installed-library
/// observation.
///
/// The adapter retains one complete bounded replacement. Cross-thread native
/// delivery enters a one-slot coalescing mailbox, and subscriber fanout is
/// finite. Commands are forwarded exactly and never mutate Swift state
/// optimistically.
@MainActor
public final class RuntimeWorkbenchLibraryManager:
    WorkbenchLibraryManaging
{
    private struct Subscriber {
        let receive: @MainActor (WorkbenchLibraryUpdate) -> Void
    }

    private static let maximumSubscribers = 16

    private let native: any RuntimeWorkbenchNativeLibraryService
    private let mailbox: RuntimeWorkbenchLibraryMailbox
    private var nativeObservation:
        (any RuntimeWorkbenchNativeLibraryObservation)?
    private var observationFailureReason: String?
    private var current: WorkbenchLibrarySnapshot
    private var subscribers: [UUID: Subscriber] = [:]

    public private(set) var latestAdmissionRefusal:
        RuntimeWorkbenchLibraryAdmissionRefusal?

    public convenience init(profile: WorkbenchRuntimeProfile) {
        self.init(native: ProfileNativeLibraryService(profile: profile))
    }

    init(native: any RuntimeWorkbenchNativeLibraryService) {
        self.native = native
        current = Self.project(native.projection())
        let mailbox = RuntimeWorkbenchLibraryMailbox()
        self.mailbox = mailbox
        nativeObservation = nil
        observationFailureReason = nil
        mailbox.bind { [weak self] update in
            self?.receive(update)
        }
        do {
            nativeObservation = try native.observe { [mailbox] update in
                mailbox.offer(update)
            }
        } catch {
            let reason =
                "Installed-library observation was refused: "
                + Self.displaySafeReason(
                    error.localizedDescription,
                    fallback: "The native observer was unavailable."
                )
            observationFailureReason = reason
            current = Self.unavailableSnapshot(
                revision: current.revision,
                reason: reason
            )
        }
    }

    public func subscribe(
        receive: @escaping @MainActor (WorkbenchLibraryUpdate) -> Void
    ) -> any WorkbenchLibrarySubscription {
        guard subscribers.count < Self.maximumSubscribers else {
            latestAdmissionRefusal = .subscriberCapacity(
                maximum: Self.maximumSubscribers
            )
            receive(.authoritative(current))
            return RuntimeWorkbenchLibrarySubscription(cancellation: {})
        }

        let identifier = UUID()
        subscribers[identifier] = Subscriber(receive: receive)
        receive(.authoritative(current))
        return RuntimeWorkbenchLibrarySubscription { [weak self] in
            self?.subscribers.removeValue(forKey: identifier)
        }
    }

    public func refresh() -> WorkbenchLibrarySnapshot {
        let refreshed: WorkbenchLibrarySnapshot
        if let observationFailureReason {
            refreshed = Self.unavailableSnapshot(
                revision: native.projection().revision,
                reason: observationFailureReason
            )
        } else {
            refreshed = Self.project(native.projection())
        }
        if refreshed.revision > current.revision {
            current = refreshed
        }
        return current
    }

    public func setFilter(_ query: String) {
        native.setFilter(query)
    }

    public func suspend(sessionID: UInt64) {
        native.suspend(sessionID: sessionID)
    }

    public func resume(sessionID: UInt64) {
        native.resume(sessionID: sessionID)
    }

    public func assign(
        _ exactBuild: WorkbenchLibraryExactBuild,
        toWorkspaceID workspaceID: String
    ) {
        native.assign(
            Self.nativeExactBuild(exactBuild),
            toWorkspaceID: workspaceID
        )
    }

    public func clearAssignment(
        _ exactBuild: WorkbenchLibraryExactBuild,
        fromWorkspaceID workspaceID: String
    ) {
        native.clearAssignment(
            Self.nativeExactBuild(exactBuild),
            fromWorkspaceID: workspaceID
        )
    }

    public func uninstall(_ exactBuild: WorkbenchLibraryExactBuild) {
        native.uninstall(Self.nativeExactBuild(exactBuild))
    }

    private func receive(_ update: NativeRuntimeLibraryUpdate) {
        switch update {
        case .authoritative(let projection):
            let projected = Self.project(projection)
            guard projected.revision > current.revision else {
                return
            }
            observationFailureReason = nil
            current = projected
            for subscriber in subscribers.values {
                subscriber.receive(.authoritative(current))
            }

        case .next(
            let projection,
            let predecessorRevision,
            _
        ):
            let projected = Self.project(projection)
            guard projected.revision > current.revision else {
                return
            }
            observationFailureReason = nil
            current = projected
            for subscriber in subscribers.values {
                subscriber.receive(
                    .next(
                        current,
                        predecessorRevision: predecessorRevision
                    )
                )
            }
        }
    }

    private static func project(
        _ projection: NativeRuntimeLibraryProjection
    ) -> WorkbenchLibrarySnapshot {
        switch projection {
        case .refused(let revision, let profileClosed, let refusal):
            let reason = profileClosed
                ? "The native runtime profile is closed."
                : "Native installed-library projection was refused: "
                    + displaySafeReason(
                        refusal.localizedDescription,
                        fallback: "The projection could not be represented."
                    )
            return unavailableSnapshot(revision: revision, reason: reason)

        case .snapshot(let snapshot):
            guard !snapshot.profileClosed else {
                return unavailableSnapshot(
                    revision: snapshot.revision,
                    reason: "The native runtime profile is closed."
                )
            }
            do {
                return try project(snapshot)
            } catch {
                return unavailableSnapshot(
                    revision: snapshot.revision,
                    reason:
                        "Native installed-library projection was refused: "
                        + displaySafeReason(
                            error.localizedDescription,
                            fallback: "The projection could not be represented."
                        )
                )
            }
        }
    }

    private static func project(
        _ native: NativeRuntimeLibrarySnapshot
    ) throws -> WorkbenchLibrarySnapshot {
        let workspaces = try native.workspaces.map { workspace in
            guard
                let projected = WorkbenchLibraryWorkspace(
                    id: workspace.id,
                    displayName: workspace.id
                )
            else {
                throw RuntimeWorkbenchLibraryProjectionError.invalidWorkspace
            }
            return projected
        }
        let builds = try native.builds.map { build in
            guard
                let exactBuild = WorkbenchLibraryExactBuild(
                    manifestAuthor: build.exactBuild.manifestAuthor,
                    dTag: build.exactBuild.dTag,
                    aggregateHash: build.exactBuild.aggregateHash
                )
            else {
                throw RuntimeWorkbenchLibraryProjectionError.invalidExactBuild
            }
            let sessions = build.sessions.map {
                WorkbenchLibrarySession(
                    id: $0.id,
                    state: sessionState($0.state)
                )
            }
            guard
                let projected = WorkbenchLibraryBuild(
                    exactBuild: exactBuild,
                    title: build.title,
                    availability: availability(build.availability),
                    sessions: sessions,
                    assignedWorkspaceIDs: build.assignedWorkspaceIDs
                )
            else {
                throw RuntimeWorkbenchLibraryProjectionError.invalidBuild
            }
            return projected
        }
        let refusals = try native.refusals.map { refusal in
            guard
                let projected = WorkbenchLibraryRefusal(
                    code: refusal.code,
                    message: refusal.detail,
                    occurredAtMillis: refusal.occurredAtMillis
                )
            else {
                throw RuntimeWorkbenchLibraryProjectionError.invalidRefusal
            }
            return projected
        }
        guard
            let snapshot = WorkbenchLibrarySnapshot(
                revision: native.revision,
                availability: .available,
                filterQuery: native.filterQuery,
                totalInstalled: native.totalInstalled,
                builds: builds,
                workspaces: workspaces,
                refusals: refusals
            )
        else {
            throw RuntimeWorkbenchLibraryProjectionError.invalidSnapshot
        }
        return snapshot
    }

    private static func nativeExactBuild(
        _ exactBuild: WorkbenchLibraryExactBuild
    ) -> NativeRuntimeLibraryExactBuild {
        NativeRuntimeLibraryExactBuild(
            manifestAuthor: exactBuild.manifestAuthor,
            dTag: exactBuild.dTag,
            aggregateHash: exactBuild.aggregateHash
        )
    }

    private static func sessionState(
        _ state: NativeRuntimeLibrarySessionState
    ) -> WorkbenchLibrarySessionState {
        switch state {
        case .running:
            .running
        case .suspended:
            .suspended
        }
    }

    private static func availability(
        _ availability: NativeRuntimeLibraryBuildAvailability
    ) -> WorkbenchLibraryBuildAvailability {
        switch availability {
        case .metadataOnly:
            .metadataOnly
        case .sealedExactBytesReady:
            .sealedExactBytesReady
        }
    }

    private static func unavailableSnapshot(
        revision: UInt64,
        reason: String
    ) -> WorkbenchLibrarySnapshot {
        let safeReason = displaySafeReason(
            reason,
            fallback: "The installed-library projection is unavailable."
        )
        guard
            let snapshot = WorkbenchLibrarySnapshot(
                revision: revision,
                availability: .unavailable(reason: safeReason),
                filterQuery: "",
                totalInstalled: 0,
                builds: [],
                workspaces: [],
                refusals: []
            )
        else {
            preconditionFailure(
                "The fixed unavailable library snapshot must remain valid"
            )
        }
        return snapshot
    }

    private static func displaySafeReason(
        _ reason: String,
        fallback: String
    ) -> String {
        guard
            !reason.isEmpty,
            reason.utf8.count
                <= WorkbenchLibraryLimits.maximumRefusalMessageUTF8Bytes,
            !reason.unicodeScalars.contains(where: {
                CharacterSet.controlCharacters.contains($0)
            })
        else {
            return fallback
        }
        return reason
    }

    deinit {
        nativeObservation?.cancel()
        mailbox.close()
    }
}

@MainActor
private final class RuntimeWorkbenchLibrarySubscription:
    WorkbenchLibrarySubscription
{
    private var cancellation: (@MainActor @Sendable () -> Void)?

    init(cancellation: @escaping @MainActor @Sendable () -> Void) {
        self.cancellation = cancellation
    }

    func cancel() {
        let cancellation = cancellation
        self.cancellation = nil
        cancellation?()
    }

    deinit {
        let cancellation = cancellation
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                cancellation?()
            }
        }
    }
}

/// One-slot replacement mailbox. There is at most one scheduled main-queue
/// drain and at most one retained update; newer complete replacements coalesce
/// older pending replacements.
private final class RuntimeWorkbenchLibraryMailbox: @unchecked Sendable {
    typealias Handler =
        @MainActor @Sendable (NativeRuntimeLibraryUpdate) -> Void

    private let lock = NSLock()
    private var handler: Handler?
    private var pending: NativeRuntimeLibraryUpdate?
    private var isScheduled = false
    private var isClosed = false

    @MainActor
    func bind(_ handler: @escaping Handler) {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        self.handler = handler
        let shouldSchedule = pending != nil && !isScheduled
        if shouldSchedule {
            isScheduled = true
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    func offer(_ update: NativeRuntimeLibraryUpdate) {
        lock.lock()
        guard !isClosed else {
            lock.unlock()
            return
        }
        pending = pending.map {
            Self.preferredPendingUpdate(current: $0, offered: update)
        } ?? update
        let shouldSchedule = handler != nil && !isScheduled
        if shouldSchedule {
            isScheduled = true
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    private static func preferredPendingUpdate(
        current: NativeRuntimeLibraryUpdate,
        offered: NativeRuntimeLibraryUpdate
    ) -> NativeRuntimeLibraryUpdate {
        let currentRevision = revision(of: current)
        let offeredRevision = revision(of: offered)
        if offeredRevision > currentRevision {
            return offered
        }
        if offeredRevision < currentRevision {
            return current
        }

        // A same-revision `next` retains predecessor metadata that can expose
        // a coalesced delivery gap. An initial authoritative replacement has
        // no such information and must not erase it merely by arriving later.
        return switch (current, offered) {
        case (.next, .authoritative):
            current
        case (.authoritative, .next):
            offered
        case (.authoritative, .authoritative), (.next, .next):
            offered
        }
    }

    private static func revision(
        of update: NativeRuntimeLibraryUpdate
    ) -> UInt64 {
        switch update {
        case .authoritative(let projection),
             .next(let projection, _, _):
            projection.revision
        }
    }

    func close() {
        lock.lock()
        isClosed = true
        pending = nil
        handler = nil
        lock.unlock()
    }

    private func scheduleDrain() {
        DispatchQueue.main.async { [weak self] in
            self?.drainOnMainQueue()
        }
    }

    private func drainOnMainQueue() {
        lock.lock()
        guard !isClosed, let update = pending, let handler else {
            isScheduled = false
            lock.unlock()
            return
        }
        pending = nil
        lock.unlock()

        MainActor.assumeIsolated {
            handler(update)
        }

        lock.lock()
        let shouldSchedule = !isClosed && pending != nil && self.handler != nil
        if !shouldSchedule {
            isScheduled = false
        }
        lock.unlock()
        if shouldSchedule {
            scheduleDrain()
        }
    }

    deinit {
        close()
    }
}
