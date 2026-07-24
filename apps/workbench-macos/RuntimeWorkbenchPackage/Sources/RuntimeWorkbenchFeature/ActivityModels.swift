import Foundation

public enum ActivityLimits {
    public static let maximumFacts = 256
    public static let maximumDetailFieldsPerFact = 24
    public static let maximumFactUTF8Bytes = 32 * 1_024
    public static let maximumSnapshotUTF8Bytes = 2 * 1_024 * 1_024
    public static let maximumIdentifierUTF8Bytes = 512
    public static let maximumDisplayFieldUTF8Bytes = 4 * 1_024
    public static let maximumDetailKeyUTF8Bytes = 128
    public static let maximumDetailValueUTF8Bytes = 8 * 1_024
}

/// The immutable build identity used to scope every activity projection.
///
/// This is deliberately more specific than a napplet coordinate: two builds
/// from the same publisher and d-tag never share activity or diagnostics.
public struct ActivityExactBuildScope: Hashable, Sendable {
    public let manifestAuthor: String
    public let dTag: String
    public let aggregateHash: String

    public init?(
        manifestAuthor: String,
        dTag: String,
        aggregateHash: String
    ) {
        let fields = [manifestAuthor, dTag, aggregateHash]
        guard fields.allSatisfy({
            !$0.isEmpty
                && $0.utf8.count <= ActivityLimits.maximumIdentifierUTF8Bytes
        }) else {
            return nil
        }

        self.manifestAuthor = manifestAuthor
        self.dTag = dTag
        self.aggregateHash = aggregateHash
    }
}

public enum ActivitySeverity: String, CaseIterable, Hashable, Sendable {
    case debug
    case information
    case warning
    case error

    public var title: String {
        switch self {
        case .debug: "Debug"
        case .information: "Information"
        case .warning: "Warning"
        case .error: "Error"
        }
    }
}

public enum ActivityCategory: String, CaseIterable, Hashable, Sendable {
    case provider
    case session
    case resource
    case receipt
    case recovery

    public var title: String {
        switch self {
        case .provider: "Providers"
        case .session: "Sessions"
        case .resource: "Resources"
        case .receipt: "Receipts"
        case .recovery: "Recovery"
        }
    }
}

/// A semantic row type supplied by the runtime-owned activity projection.
///
/// Native code does not infer success, refusal, recovery, or receipt state from
/// strings. It renders the classification already decided by the runtime.
public enum ActivityFactKind: String, CaseIterable, Hashable, Sendable {
    case providerCall
    case providerRefusal
    case activeSession
    case activeBinding
    case activeResource
    case pendingReceipt
    case crash
    case recovery

    public var title: String {
        switch self {
        case .providerCall: "Provider call"
        case .providerRefusal: "Provider refusal"
        case .activeSession: "Active session"
        case .activeBinding: "Active binding"
        case .activeResource: "Active resource"
        case .pendingReceipt: "Pending receipt"
        case .crash: "Crash"
        case .recovery: "Recovery"
        }
    }
}

public struct ActivityDetailField: Identifiable, Equatable, Sendable {
    public let key: String
    public let value: String

    public var id: String {
        key
    }

    public init?(key: String, value: String) {
        guard !key.isEmpty,
              key.utf8.count <= ActivityLimits.maximumDetailKeyUTF8Bytes,
              value.utf8.count <= ActivityLimits.maximumDetailValueUTF8Bytes
        else {
            return nil
        }

        self.key = ActivitySecretRedactor.displayText(key)
        self.value = ActivitySecretRedactor.detailValue(
            key: key,
            value: value
        )
    }
}

public struct ActivityFact: Identifiable, Equatable, Sendable {
    public let id: String
    public let scope: ActivityExactBuildScope
    public let ordinal: UInt64
    public let severity: ActivitySeverity
    public let category: ActivityCategory
    public let kind: ActivityFactKind
    public let title: String
    public let summary: String
    public let evidenceSummary: String?
    public let detailFields: [ActivityDetailField]

    public init?(
        id: String,
        scope: ActivityExactBuildScope,
        ordinal: UInt64,
        severity: ActivitySeverity,
        category: ActivityCategory,
        kind: ActivityFactKind,
        title: String,
        summary: String,
        evidenceSummary: String? = nil,
        detailFields: [ActivityDetailField] = []
    ) {
        let displayFields = [id, title, summary, evidenceSummary ?? ""]
        guard !id.isEmpty,
              !title.isEmpty,
              detailFields.count
                <= ActivityLimits.maximumDetailFieldsPerFact,
              Set(detailFields.map(\.key)).count == detailFields.count,
              displayFields.allSatisfy({
                  $0.utf8.count <= ActivityLimits.maximumDisplayFieldUTF8Bytes
              })
        else {
            return nil
        }

        let byteCount = displayFields.reduce(0) { $0 + $1.utf8.count }
            + detailFields.reduce(0) {
                $0 + $1.key.utf8.count + $1.value.utf8.count
            }
        guard byteCount <= ActivityLimits.maximumFactUTF8Bytes else {
            return nil
        }

        self.id = ActivitySecretRedactor.displayText(id)
        self.scope = scope
        self.ordinal = ordinal
        self.severity = severity
        self.category = category
        self.kind = kind
        self.title = ActivitySecretRedactor.displayText(title)
        self.summary = ActivitySecretRedactor.displayText(summary)
        self.evidenceSummary = evidenceSummary.map(
            ActivitySecretRedactor.displayText
        )
        self.detailFields = detailFields
    }

    fileprivate var activityUTF8ByteCount: Int {
        [id, title, summary, evidenceSummary ?? ""].reduce(0) {
            $0 + $1.utf8.count
        } + detailFields.reduce(0) {
            $0 + $1.key.utf8.count + $1.value.utf8.count
        }
    }
}

/// Counts are supplied by the runtime projection and never derived from the
/// bounded recent-fact window.
public struct ActivityInventorySummary: Equatable, Sendable {
    public static let maximumActiveSessions = 32
    public static let maximumActiveBindings = 256
    public static let maximumActiveResources = 1_024
    public static let maximumPendingReceipts = 512

    public let activeSessions: Int
    public let activeBindings: Int
    public let activeResources: Int
    public let pendingReceipts: Int

    public init?(
        activeSessions: Int,
        activeBindings: Int,
        activeResources: Int,
        pendingReceipts: Int
    ) {
        guard (0...Self.maximumActiveSessions).contains(activeSessions),
              (0...Self.maximumActiveBindings).contains(activeBindings),
              (0...Self.maximumActiveResources).contains(activeResources),
              (0...Self.maximumPendingReceipts).contains(pendingReceipts)
        else {
            return nil
        }

        self.activeSessions = activeSessions
        self.activeBindings = activeBindings
        self.activeResources = activeResources
        self.pendingReceipts = pendingReceipts
    }

    public static let empty = ActivityInventorySummary(
        activeSessions: 0,
        activeBindings: 0,
        activeResources: 0,
        pendingReceipts: 0
    )!
}

/// A bounded, screen-shaped replacement projection.
public struct ActivitySnapshot: Equatable, Sendable {
    public let scope: ActivityExactBuildScope
    public let revision: UInt64
    public let inventory: ActivityInventorySummary
    public let facts: [ActivityFact]
    public let omittedFactCount: UInt64

    public init?(
        scope: ActivityExactBuildScope,
        revision: UInt64,
        inventory: ActivityInventorySummary,
        facts: [ActivityFact],
        omittedFactCount: UInt64 = 0
    ) {
        guard facts.count <= ActivityLimits.maximumFacts,
              facts.allSatisfy({ $0.scope == scope }),
              Set(facts.map(\.id)).count == facts.count,
              facts.reduce(0, {
                  $0 + $1.activityUTF8ByteCount
              }) <= ActivityLimits.maximumSnapshotUTF8Bytes
        else {
            return nil
        }

        self.scope = scope
        self.revision = revision
        self.inventory = inventory
        self.facts = facts
        self.omittedFactCount = omittedFactCount
    }
}

public struct ActivityUpdateGap: Equatable, Sendable {
    public let expectedPredecessorRevision: UInt64
    public let receivedPredecessorRevision: UInt64
    public let receivedRevision: UInt64

    public init(
        expectedPredecessorRevision: UInt64,
        receivedPredecessorRevision: UInt64,
        receivedRevision: UInt64
    ) {
        self.expectedPredecessorRevision = expectedPredecessorRevision
        self.receivedPredecessorRevision = receivedPredecessorRevision
        self.receivedRevision = receivedRevision
    }
}

enum ActivitySecretRedactor {
    static let redacted = "[REDACTED]"

    private static let sensitiveKeyFragments = [
        "authorization",
        "cookie",
        "credential",
        "nsec",
        "password",
        "privatekey",
        "secret",
        "token",
    ]

    private static let sensitiveValueMarkers = [
        "authorization:",
        "bearer ",
        "cookie=",
        "nsec1",
        "password=",
        "private_key=",
        "privatekey=",
        "secret=",
        "token=",
    ]

    static func detailValue(key: String, value: String) -> String {
        let normalizedKey = key.lowercased().filter(\.isLetter)
        guard !sensitiveKeyFragments.contains(where: {
            normalizedKey.contains($0)
        }) else {
            return redacted
        }
        return displayText(value)
    }

    static func displayText(_ value: String) -> String {
        let lowercased = value.lowercased()
        guard !sensitiveValueMarkers.contains(where: lowercased.contains)
        else {
            return redacted
        }
        return value
    }
}
