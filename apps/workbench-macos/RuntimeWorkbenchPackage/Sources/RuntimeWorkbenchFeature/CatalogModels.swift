import Foundation

public enum CatalogLimits {
    public static let maximumQueryUTF8Bytes = 256
    public static let maximumCoordinateUTF8Bytes = 2_048
    public static let maximumEntryUTF8Bytes = 16_384
    public static let maximumSearchPageUTF8Bytes = 512 * 1_024
    public static let maximumReviewUTF8Bytes = 128 * 1_024
    public static let maximumFieldUTF8Bytes = 16_384
    public static let maximumEntriesPerPage = 100
    public static let maximumSourcesPerReview = 16
    public static let maximumDomainsPerClass = 64
    public static let maximumPlatformRows = 16
    public static let maximumWarningsPerReview = 32
    public static let maximumBrowseSources = 64
    public static let maximumBrowseShortfalls = 3
    public static let maximumSourceLabelUTF8Bytes = 512
}

public struct CatalogSearchRequest: Equatable, Sendable {
    public let query: String

    public init?(query: String) {
        guard query.utf8.count <= CatalogLimits.maximumQueryUTF8Bytes else {
            return nil
        }
        self.query = query
    }
}

public struct CatalogManualCoordinateRequest: Equatable, Sendable {
    public let coordinate: String

    public init?(coordinate: String) {
        guard !coordinate.isEmpty,
              coordinate.utf8.count <= CatalogLimits.maximumCoordinateUTF8Bytes
        else {
            return nil
        }
        self.coordinate = coordinate
    }
}

public struct CatalogPublisher: Equatable, Sendable {
    public let displayName: String?
    public let publicKey: String

    public init(displayName: String?, publicKey: String) {
        self.displayName = displayName
        self.publicKey = publicKey
    }

    public var visibleName: String {
        displayName ?? publicKey
    }
}

public enum CatalogCompatibilitySummary: Equatable, Sendable {
    case unreviewed
    case compatible
    case incompatible(reason: String)
    case unknown(reason: String)

    public var title: String {
        switch self {
        case .unreviewed:
            "Review required"
        case .compatible:
            "Compatible"
        case .incompatible:
            "Incompatible"
        case .unknown:
            "Compatibility unknown"
        }
    }

    public var detail: String? {
        switch self {
        case .unreviewed:
            "Verify the exact signed manifest before installation."
        case .compatible:
            nil
        case let .incompatible(reason), let .unknown(reason):
            reason
        }
    }
}

public struct CatalogEntry: Identifiable, Equatable, Sendable {
    public let id: String
    public let title: String
    public let summary: String
    public let publisher: CatalogPublisher
    public let coordinate: String
    public let compatibility: CatalogCompatibilitySummary

    public init?(
        id: String,
        title: String,
        summary: String,
        publisher: CatalogPublisher,
        coordinate: String,
        compatibility: CatalogCompatibilitySummary
    ) {
        let fields = [
            id,
            title,
            summary,
            publisher.displayName ?? "",
            publisher.publicKey,
            coordinate,
            compatibility.detail ?? "",
        ]
        guard fields.allSatisfy({
            $0.utf8.count <= CatalogLimits.maximumFieldUTF8Bytes
        }),
            fields.reduce(0, { $0 + $1.utf8.count })
                <= CatalogLimits.maximumEntryUTF8Bytes,
            coordinate.utf8.count <= CatalogLimits.maximumCoordinateUTF8Bytes
        else {
            return nil
        }

        self.id = id
        self.title = title
        self.summary = summary
        self.publisher = publisher
        self.coordinate = coordinate
        self.compatibility = compatibility
    }

    fileprivate var catalogUTF8ByteCount: Int {
        [
            id,
            title,
            summary,
            publisher.displayName ?? "",
            publisher.publicKey,
            coordinate,
            compatibility.detail ?? "",
        ].reduce(0) { $0 + $1.utf8.count }
    }
}

/// The authority from which a catalog page was projected.
///
/// A live page is one finite NMP observation, not a globally complete network
/// result. The offline fixture is kept solely for deterministic previews and
/// UI automation.
public enum CatalogBrowseScope: Equatable, Sendable {
    case liveNMPWindow
    case offlineFixture
}

public enum CatalogBrowseSourceStatus: Equatable, Sendable {
    case requesting
    case connecting
    case disconnected
    case awaitingAuthentication
    case authenticationDenied
    case error
}

public enum CatalogBrowseAccessContext: Equatable, Sendable {
    case `public`
    case nip42(publicKey: String)
}

public struct CatalogBrowseSourceEvidence:
    Identifiable,
    Equatable,
    Sendable
{
    public let id: String
    public let source: String
    public let access: CatalogBrowseAccessContext
    public let status: CatalogBrowseSourceStatus
    public let reconciledThrough: UInt64?

    public init?(
        id: String,
        source: String,
        access: CatalogBrowseAccessContext,
        status: CatalogBrowseSourceStatus,
        reconciledThrough: UInt64?
    ) {
        let accessPublicKey: String
        switch access {
        case .public:
            accessPublicKey = ""
        case let .nip42(publicKey):
            accessPublicKey = publicKey
        }
        guard
            !id.isEmpty,
            id.utf8.count <= CatalogLimits.maximumSourceLabelUTF8Bytes,
            !source.isEmpty,
            source.utf8.count <= CatalogLimits.maximumSourceLabelUTF8Bytes,
            accessPublicKey.utf8.count
                <= CatalogLimits.maximumSourceLabelUTF8Bytes,
            id.catalogIsControlFree,
            source.catalogIsControlFree,
            accessPublicKey.catalogIsControlFree
        else {
            return nil
        }
        self.id = id
        self.source = source
        self.access = access
        self.status = status
        self.reconciledThrough = reconciledThrough
    }
}

public enum CatalogBrowseShortfall: Hashable, Sendable {
    case noPlannedSource
    case noResolvedDemand
    case localLimit
}

public enum CatalogBrowseWindowState: Equatable, Sendable {
    case idle
    case requesting
    case returned(addedRows: UInt64)
    case atBound(maximumRows: UInt64)
    case unknown
}

/// Bounded evidence displayed beside every page.
///
/// `locallyFilteredRows` is supplied by the Rust projection; Swift never
/// interprets relay completeness or derives routing/search claims from rows.
public struct CatalogBrowseEvidence: Equatable, Sendable {
    public let scope: CatalogBrowseScope
    public let queryWasLocalFilter: Bool
    public let locallyFilteredRows: UInt
    public let projectedRows: UInt
    public let projectionLimitedRows: UInt
    public let refusedRows: UInt
    public let window: CatalogBrowseWindowState
    public let sourceEvidence: [CatalogBrowseSourceEvidence]
    public let shortfalls: [CatalogBrowseShortfall]

    public init?(
        scope: CatalogBrowseScope,
        queryWasLocalFilter: Bool,
        locallyFilteredRows: UInt,
        projectedRows: UInt,
        projectionLimitedRows: UInt,
        refusedRows: UInt,
        window: CatalogBrowseWindowState,
        sourceEvidence: [CatalogBrowseSourceEvidence],
        shortfalls: [CatalogBrowseShortfall]
    ) {
        guard
            projectedRows <= UInt(CatalogLimits.maximumEntriesPerPage),
            queryWasLocalFilter || locallyFilteredRows == 0,
            sourceEvidence.count <= CatalogLimits.maximumBrowseSources,
            Set(sourceEvidence.map(\.id)).count == sourceEvidence.count,
            shortfalls.count <= CatalogLimits.maximumBrowseShortfalls,
            Set(shortfalls).count == shortfalls.count
        else {
            return nil
        }
        self.scope = scope
        self.queryWasLocalFilter = queryWasLocalFilter
        self.locallyFilteredRows = locallyFilteredRows
        self.projectedRows = projectedRows
        self.projectionLimitedRows = projectionLimitedRows
        self.refusedRows = refusedRows
        self.window = window
        self.sourceEvidence = sourceEvidence
        self.shortfalls = shortfalls
    }
}

public struct CatalogSearchPage: Equatable, Sendable {
    public let entries: [CatalogEntry]
    public let hasMore: Bool
    public let evidence: CatalogBrowseEvidence

    public init?(
        entries: [CatalogEntry],
        hasMore: Bool,
        evidence: CatalogBrowseEvidence
    ) {
        guard
            entries.count <= CatalogLimits.maximumEntriesPerPage,
            evidence.projectedRows == UInt(entries.count),
            hasMore == (evidence.projectionLimitedRows > 0),
              entries.reduce(0, { $0 + $1.catalogUTF8ByteCount })
                <= CatalogLimits.maximumSearchPageUTF8Bytes
        else {
            return nil
        }
        self.entries = entries
        self.hasMore = hasMore
        self.evidence = evidence
    }
}

public struct CatalogIssue: Equatable, Sendable {
    public let title: String
    public let message: String

    public init(title: String, message: String) {
        self.title = title
        self.message = message
    }
}

public enum CatalogSearchResponse: Equatable, Sendable {
    case page(CatalogSearchPage)
    case unavailable(CatalogIssue)
}

public enum CatalogReviewTarget: Equatable, Sendable {
    case entryID(String)
    case manualCoordinate(CatalogManualCoordinateRequest)
}

public enum CatalogSourceKind: String, Equatable, Sendable {
    case manifestEvent = "Manifest event"
    case artifact = "Verified artifact"
    case approvedCatalog = "Approved catalog"
    case verifiedArtifactIndex = "Verified artifact index"
}

public struct CatalogSourceProvenance: Identifiable, Equatable, Sendable {
    public let id: String
    public let kind: CatalogSourceKind
    public let source: String
    public let evidence: String

    public init(id: String, kind: CatalogSourceKind, source: String, evidence: String) {
        self.id = id
        self.kind = kind
        self.source = source
        self.evidence = evidence
    }
}

public enum CatalogPlatformStatus: Equatable, Sendable {
    case compatible
    case incompatible
    case unavailable
}

public struct CatalogPlatformCompatibility: Identifiable, Equatable, Sendable {
    public let id: String
    public let platform: String
    public let status: CatalogPlatformStatus
    public let detail: String

    public init(
        id: String,
        platform: String,
        status: CatalogPlatformStatus,
        detail: String
    ) {
        self.id = id
        self.platform = platform
        self.status = status
        self.detail = detail
    }
}

public enum CatalogWarningSeverity: Equatable, Sendable {
    case information
    case caution
    case blocking
}

public struct CatalogWarning: Identifiable, Equatable, Sendable {
    public let id: String
    public let severity: CatalogWarningSeverity
    public let message: String

    public init(id: String, severity: CatalogWarningSeverity, message: String) {
        self.id = id
        self.severity = severity
        self.message = message
    }
}

public enum CatalogUpdateRelationship: Equatable, Sendable {
    case unknown(reason: String)
    case firstInstall
    case sameBuild
    case update(installedHash: String)
    case rollback(installedHash: String)
    case differentBuild(installedHash: String)

    public var title: String {
        switch self {
        case .unknown:
            "Install relationship unavailable"
        case .firstInstall:
            "New install"
        case .sameBuild:
            "Already installed"
        case .update:
            "Update"
        case .rollback:
            "Rollback"
        case .differentBuild:
            "Different build"
        }
    }

    public var installedHash: String? {
        switch self {
        case .unknown, .firstInstall, .sameBuild:
            nil
        case let .update(installedHash),
             let .rollback(installedHash),
             let .differentBuild(installedHash):
            installedHash
        }
    }

    public var detail: String? {
        guard case let .unknown(reason) = self else {
            return nil
        }
        return reason
    }
}

public struct CatalogInstallReview: Identifiable, Equatable, Sendable {
    public let id: String
    public let title: String
    public let publisher: CatalogPublisher
    public let coordinate: String
    public let exactAggregateHash: String
    public let sources: [CatalogSourceProvenance]
    public let requiredDomains: [String]
    public let optionalDomains: [String]
    public let platformCompatibility: [CatalogPlatformCompatibility]
    public let warnings: [CatalogWarning]
    public let updateRelationship: CatalogUpdateRelationship
    public let canInstall: Bool

    public init?(
        id: String,
        title: String,
        publisher: CatalogPublisher,
        coordinate: String,
        exactAggregateHash: String,
        sources: [CatalogSourceProvenance],
        requiredDomains: [String],
        optionalDomains: [String],
        platformCompatibility: [CatalogPlatformCompatibility],
        warnings: [CatalogWarning],
        updateRelationship: CatalogUpdateRelationship,
        canInstall: Bool
    ) {
        let identityFields: [String] = [
            id,
            title,
            publisher.displayName ?? "",
            publisher.publicKey,
            coordinate,
            exactAggregateHash,
        ]
        let sourceFields: [String] = sources.flatMap {
            [$0.id, $0.source, $0.evidence]
        }
        let platformFields: [String] = platformCompatibility.flatMap {
            [$0.id, $0.platform, $0.detail]
        }
        let warningFields: [String] = warnings.flatMap {
            [$0.id, $0.message]
        }
        let updateRelationshipFields: [String] = [
            updateRelationship.installedHash ?? "",
            updateRelationship.detail ?? "",
        ]
        let textFields: [String] = identityFields
            + sourceFields
            + requiredDomains
            + optionalDomains
            + platformFields
            + warningFields
            + updateRelationshipFields

        guard coordinate.utf8.count <= CatalogLimits.maximumCoordinateUTF8Bytes,
              textFields.allSatisfy({
                  $0.utf8.count <= CatalogLimits.maximumFieldUTF8Bytes
              }),
              textFields.reduce(0, { $0 + $1.utf8.count })
                <= CatalogLimits.maximumReviewUTF8Bytes,
              sources.count <= CatalogLimits.maximumSourcesPerReview,
              requiredDomains.count <= CatalogLimits.maximumDomainsPerClass,
              optionalDomains.count <= CatalogLimits.maximumDomainsPerClass,
              platformCompatibility.count <= CatalogLimits.maximumPlatformRows,
              warnings.count <= CatalogLimits.maximumWarningsPerReview
        else {
            return nil
        }

        self.id = id
        self.title = title
        self.publisher = publisher
        self.coordinate = coordinate
        self.exactAggregateHash = exactAggregateHash
        self.sources = sources
        self.requiredDomains = requiredDomains
        self.optionalDomains = optionalDomains
        self.platformCompatibility = platformCompatibility
        self.warnings = warnings
        self.updateRelationship = updateRelationship
        self.canInstall = canInstall
    }
}

public enum CatalogReviewResponse: Equatable, Sendable {
    case ready(CatalogInstallReview)
    case unavailable(CatalogIssue)
}

public struct CatalogInstallConfirmation: Equatable, Sendable {
    public let reviewID: String
    public let publisherPublicKey: String
    public let coordinate: String
    public let exactAggregateHash: String

    public init(review: CatalogInstallReview) {
        reviewID = review.id
        publisherPublicKey = review.publisher.publicKey
        coordinate = review.coordinate
        exactAggregateHash = review.exactAggregateHash
    }
}

public struct CatalogInstalledBuild: Equatable, Sendable {
    public let title: String
    public let manifestAuthor: String
    public let dTag: String
    public let exactAggregateHash: String

    public init?(
        title: String,
        manifestAuthor: String,
        dTag: String,
        exactAggregateHash: String
    ) {
        let fields = [title, manifestAuthor, dTag, exactAggregateHash]
        guard
            !title.isEmpty,
            !dTag.isEmpty,
            manifestAuthor.catalogIsLowercaseHexDigest,
            exactAggregateHash.catalogIsLowercaseHexDigest,
            dTag.utf8.count <= CatalogLimits.maximumCoordinateUTF8Bytes,
            fields.allSatisfy({
                $0.utf8.count <= CatalogLimits.maximumFieldUTF8Bytes
                    && $0.catalogIsControlFree
            })
        else {
            return nil
        }
        self.title = title
        self.manifestAuthor = manifestAuthor
        self.dTag = dTag
        self.exactAggregateHash = exactAggregateHash
    }
}

public enum CatalogInstallResponse: Equatable, Sendable {
    case installed(CatalogInstalledBuild)
    case refused(CatalogIssue)
}

private extension String {
    var catalogIsControlFree: Bool {
        !unicodeScalars.contains {
            CharacterSet.controlCharacters.contains($0)
        }
    }

    var catalogIsLowercaseHexDigest: Bool {
        utf8.count == 64
            && utf8.allSatisfy { byte in
                (48 ... 57).contains(byte) || (97 ... 102).contains(byte)
            }
    }
}
