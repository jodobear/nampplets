import Foundation
import NMPNativeRuntimeApple

public enum InspectorRelayLimits {
    public static let maximumRelays = 64
    public static let maximumSubscriptionsPerRelay = 64
    public static let maximumLaneRows = 16
    public static let maximumEventKindRows = 64
    public static let maximumSupportedNips = 128
    public static let maximumDroppedMergeRules = 32
    public static let maximumRelayUTF8Bytes = 2_048
    public static let maximumFilterUTF8Bytes = 16_384
    public static let maximumDetailUTF8Bytes = 2_048
}

public enum InspectorRelayLane: String, CaseIterable, Equatable, Sendable {
    case nip65Write
    case nip65Read
    case hint
    case provenance
    case userConfigured
    case indexerDiscovery
    case groupHost
    case dmInbox
    case appRelay
    case fallback
    case explicitPinned

    public var title: String {
        switch self {
        case .nip65Write: "NIP-65 write"
        case .nip65Read: "NIP-65 read"
        case .hint: "Hint"
        case .provenance: "Provenance"
        case .userConfigured: "User configured"
        case .indexerDiscovery: "Indexer discovery"
        case .groupHost: "Group host"
        case .dmInbox: "DM inbox"
        case .appRelay: "App relay"
        case .fallback: "Fallback"
        case .explicitPinned: "Explicit pinned"
        }
    }

    init(_ native: NativeRuntimeRelayLane) {
        switch native {
        case .nip65Write: self = .nip65Write
        case .nip65Read: self = .nip65Read
        case .hint: self = .hint
        case .provenance: self = .provenance
        case .userConfigured: self = .userConfigured
        case .indexerDiscovery: self = .indexerDiscovery
        case .groupHost: self = .groupHost
        case .dmInbox: self = .dmInbox
        case .appRelay: self = .appRelay
        case .fallback: self = .fallback
        case .explicitPinned: self = .explicitPinned
        }
    }
}

public enum InspectorRelayAccess: Equatable, Sendable {
    case `public`
    case nip42(publicKey: String)

    init(_ native: NativeRuntimeRelayAccess) {
        switch native {
        case .public:
            self = .public
        case let .nip42(publicKey):
            self = .nip42(publicKey: publicKey)
        }
    }

    public var title: String {
        switch self {
        case .public: "Public"
        case .nip42: "NIP-42 authenticated"
        }
    }
}

public struct InspectorRelayCoverage: Equatable, Sendable {
    public let fromSeconds: UInt64
    public let throughSeconds: UInt64

    init(_ native: NativeRuntimeRelayCoverage) {
        fromSeconds = native.fromSeconds
        throughSeconds = native.throughSeconds
    }
}

public struct InspectorRelaySubscription: Identifiable, Equatable, Sendable {
    public let id: String
    public let filter: String
    /// `nil` means unproven — never render as zero coverage.
    public let coverage: InspectorRelayCoverage?

    init?(_ native: NativeRuntimeRelaySubscription, ordinal: Int) {
        guard
            !native.filter.isEmpty,
            native.filter.utf8.count <= InspectorRelayLimits.maximumFilterUTF8Bytes
        else {
            return nil
        }
        id = "\(ordinal):\(native.filter)"
        filter = native.filter
        coverage = native.coverage.map(InspectorRelayCoverage.init)
    }
}

public struct InspectorRelayLaneCount: Identifiable, Equatable, Sendable {
    public var id: InspectorRelayLane { lane }
    public let lane: InspectorRelayLane
    public let wireSubscriptions: UInt64

    init(_ native: NativeRuntimeRelayLaneCount) {
        lane = InspectorRelayLane(native.lane)
        wireSubscriptions = native.wireSubscriptions
    }
}

public struct InspectorRelayKindCount: Identifiable, Equatable, Sendable {
    public var id: UInt16 { kind }
    public let kind: UInt16
    public let events: UInt64

    init(_ native: NativeRuntimeRelayKindCount) {
        kind = native.kind
        events = native.events
    }
}

public struct InspectorRelayDiagnostics: Identifiable, Equatable, Sendable {
    public let id: String
    public let relay: String
    public let access: InspectorRelayAccess
    public let wireSubscriptionCount: UInt64
    public let authorsServed: UInt64
    public let lanes: [InspectorRelayLaneCount]
    public let omittedLanes: UInt64
    public let subscriptions: [InspectorRelaySubscription]
    public let omittedSubscriptions: UInt64
    public let eventsByKind: [InspectorRelayKindCount]
    public let omittedKinds: UInt64
    public let supportedNips: [UInt16]?
    public let omittedSupportedNips: UInt64
    public let nip11DocumentRevision: String?
    public let nip11Freshness: String?
    public let nip11LastError: String?
    public let nip77Advertisement: String
    public let nip77Behavior: String
    public let nip77Handoff: String

    init?(_ native: NativeRuntimeRelayDiagnostics) {
        guard
            !native.relay.isEmpty,
            native.relay.utf8.count <= InspectorRelayLimits.maximumRelayUTF8Bytes
        else {
            return nil
        }
        relay = native.relay
        id = native.relay
        access = InspectorRelayAccess(native.access)
        wireSubscriptionCount = native.wireSubscriptionCount
        authorsServed = native.authorsServed
        lanes = Array(
            native.lanes.map(InspectorRelayLaneCount.init)
                .prefix(InspectorRelayLimits.maximumLaneRows)
        )
        omittedLanes = native.omittedLanes
            + UInt64(max(native.lanes.count - lanes.count, 0))
        let boundedSubscriptions = native.subscriptions
            .prefix(InspectorRelayLimits.maximumSubscriptionsPerRelay)
            .enumerated()
            .compactMap { ordinal, subscription in
                InspectorRelaySubscription(subscription, ordinal: ordinal)
            }
        subscriptions = boundedSubscriptions
        omittedSubscriptions = native.omittedSubscriptions
            + UInt64(max(native.subscriptions.count - boundedSubscriptions.count, 0))
        eventsByKind = Array(
            native.eventsByKind.map(InspectorRelayKindCount.init)
                .prefix(InspectorRelayLimits.maximumEventKindRows)
        )
        omittedKinds = native.omittedKinds
            + UInt64(max(native.eventsByKind.count - eventsByKind.count, 0))
        supportedNips = native.supportedNips.map {
            Array($0.prefix(InspectorRelayLimits.maximumSupportedNips))
        }
        omittedSupportedNips = native.omittedSupportedNips
        nip11DocumentRevision = Self.bounded(native.nip11DocumentRevision)
        nip11Freshness = Self.bounded(native.nip11Freshness)
        nip11LastError = Self.bounded(native.nip11LastError)
        nip77Advertisement = native.nip77Advertisement
        nip77Behavior = native.nip77Behavior
        nip77Handoff = native.nip77Handoff
    }

    private static func bounded(_ value: String?) -> String? {
        guard let value, value.utf8.count <= InspectorRelayLimits.maximumDetailUTF8Bytes else {
            return nil
        }
        return value
    }
}

public struct InspectorRelayRefusal: Equatable, Sendable {
    public let code: String
    public let detail: String

    init(code: String, detail: String) {
        self.code = code
        self.detail = detail
    }
}

/// Screen-shaped mirror of `NativeRuntimeRelayDiagnosticsSnapshot`. A relay
/// row that fails bounding is dropped and folded into `omittedRelays` rather
/// than invalidating the whole snapshot — partial relay visibility is more
/// useful than none in an inspector.
public struct InspectorRelayDiagnosticsSnapshot: Equatable, Sendable {
    public let revision: UInt64
    /// `false` means the observation is not currently open: an empty
    /// `relays` here means "not accounted", never "no relay planned".
    public let observing: Bool
    public let relays: [InspectorRelayDiagnostics]
    public let omittedRelays: UInt64
    public let uncoveredAuthorCount: UInt64
    public let droppedMergeRules: [String]
    public let omittedDroppedMergeRules: UInt64
    public let discoveredPrivateRelaysRejected: UInt64
    public let sessionsRejectedOverCap: UInt64
    public let storeDegraded: String?
    public let transportDegraded: String?
    public let failure: InspectorRelayRefusal?

    init(_ native: NativeRuntimeRelayDiagnosticsSnapshot) {
        revision = native.revision
        observing = native.observing
        let boundedRelays = native.relays
            .prefix(InspectorRelayLimits.maximumRelays)
            .compactMap(InspectorRelayDiagnostics.init)
        relays = boundedRelays
        omittedRelays = native.omittedRelays
            + UInt64(max(native.relays.count - boundedRelays.count, 0))
        uncoveredAuthorCount = native.uncoveredAuthorCount
        droppedMergeRules = Array(
            native.droppedMergeRules.prefix(InspectorRelayLimits.maximumDroppedMergeRules)
        )
        omittedDroppedMergeRules = native.omittedDroppedMergeRules
            + UInt64(max(native.droppedMergeRules.count - droppedMergeRules.count, 0))
        discoveredPrivateRelaysRejected = native.discoveredPrivateRelaysRejected
        sessionsRejectedOverCap = native.sessionsRejectedOverCap
        storeDegraded = native.storeDegraded
        transportDegraded = native.transportDegraded
        failure = native.failure.map {
            InspectorRelayRefusal(code: $0.code, detail: $0.detail)
        }
    }

    public static let notObserving = InspectorRelayDiagnosticsSnapshot(
        revision: 0,
        observing: false
    )

    private init(revision: UInt64, observing: Bool) {
        self.revision = revision
        self.observing = observing
        relays = []
        omittedRelays = 0
        uncoveredAuthorCount = 0
        droppedMergeRules = []
        omittedDroppedMergeRules = 0
        discoveredPrivateRelaysRejected = 0
        sessionsRejectedOverCap = 0
        storeDegraded = nil
        transportDegraded = nil
        failure = nil
    }
}
