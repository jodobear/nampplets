import Foundation

public enum CatalogLimits {
    public static let maximumQueryUTF8Bytes = 512
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
    case compatible
    case incompatible(reason: String)
    case unknown(reason: String)

    public var title: String {
        switch self {
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

public struct CatalogSearchPage: Equatable, Sendable {
    public let entries: [CatalogEntry]
    public let hasMore: Bool

    public init?(entries: [CatalogEntry], hasMore: Bool) {
        guard entries.count <= CatalogLimits.maximumEntriesPerPage,
              entries.reduce(0, { $0 + $1.catalogUTF8ByteCount })
                <= CatalogLimits.maximumSearchPageUTF8Bytes
        else {
            return nil
        }
        self.entries = entries
        self.hasMore = hasMore
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
    case firstInstall
    case sameBuild
    case update(installedHash: String)
    case rollback(installedHash: String)
    case differentBuild(installedHash: String)

    public var title: String {
        switch self {
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
        case .firstInstall, .sameBuild:
            nil
        case let .update(installedHash),
             let .rollback(installedHash),
             let .differentBuild(installedHash):
            installedHash
        }
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
        let textFields = [
            id,
            title,
            publisher.displayName ?? "",
            publisher.publicKey,
            coordinate,
            exactAggregateHash,
        ] + sources.flatMap {
            [$0.id, $0.source, $0.evidence]
        } + requiredDomains + optionalDomains + platformCompatibility.flatMap {
            [$0.id, $0.platform, $0.detail]
        } + warnings.flatMap {
            [$0.id, $0.message]
        } + [updateRelationship.installedHash ?? ""]

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
    public let publisherPublicKey: String
    public let coordinate: String
    public let exactAggregateHash: String

    public init(
        publisherPublicKey: String,
        coordinate: String,
        exactAggregateHash: String
    ) {
        self.publisherPublicKey = publisherPublicKey
        self.coordinate = coordinate
        self.exactAggregateHash = exactAggregateHash
    }
}

public enum CatalogInstallResponse: Equatable, Sendable {
    case installed(CatalogInstalledBuild)
    case refused(CatalogIssue)
}
