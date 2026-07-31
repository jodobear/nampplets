import Foundation

struct CatalogRecord {
    let entry: CatalogEntry
    let review: CatalogInstallReview?
    let reviewIssue: CatalogIssue?
    let searchText: String

    init(
        entry: CatalogEntry,
        review: CatalogInstallReview?,
        reviewIssue: CatalogIssue?,
        searchTerms: [String]
    ) {
        self.entry = entry
        self.review = review
        self.reviewIssue = reviewIssue
        searchText = (
            [
                entry.title,
                entry.summary,
                entry.publisher.visibleName,
                entry.publisher.publicKey,
                entry.coordinate,
                entry.compatibility.title,
                entry.compatibility.detail ?? "",
            ] + searchTerms
        )
        .joined(separator: "\n")
        .lowercased()
    }
}

enum CatalogResourceError: Error, LocalizedError {
    case missing(String)
    case unexpectedBaseline
    case unexpectedPublishedFixture
    case outsideUILimits

    var errorDescription: String? {
        switch self {
        case let .missing(name):
            "Missing bundled resource \(name)."
        case .unexpectedBaseline:
            "Bundled catalog metadata does not match compatibility.lock."
        case .unexpectedPublishedFixture:
            "The bundled published fixture differs from the pinned exact build."
        case .outsideUILimits:
            "A bundled catalog entry is outside the finite UI limits."
        }
    }
}

struct PublishedIndex: Decodable {
    let classification: String
    let digest: String
    let fixtures: [PublishedFixture]
    let schema: Int
}

struct PublishedFixture: Decodable {
    let aggregateSHA256: String
    let artifactMode: String
    let coordinate: PublishedCoordinate
    let eventID: String
    let files: [PublishedFile]
    let name: String

    private enum CodingKeys: String, CodingKey {
        case aggregateSHA256 = "aggregate_sha256"
        case artifactMode = "artifact_mode"
        case coordinate
        case eventID = "event_id"
        case files
        case name
    }
}

struct PublishedCoordinate: Decodable {
    let author: String
    let dTag: String
    let kind: Int

    private enum CodingKeys: String, CodingKey {
        case author
        case dTag = "d_tag"
        case kind
    }
}

struct PublishedFile: Decodable, Equatable {
    let artifactPath: String?
    let bytes: Int
    let path: String
    let sha256: String

    private enum CodingKeys: String, CodingKey {
        case artifactPath = "artifact_path"
        case bytes
        case path
        case sha256
    }
}

struct ReferenceIndex: Decodable {
    let classification: String
    let digest: String
    let fixtures: [ReferenceFixture]
    let schema: Int
}

struct ReferenceFixture: Decodable {
    let aggregateSHA256: String
    let artifactMode: String
    let name: String
    let requires: [String]

    private enum CodingKeys: String, CodingKey {
        case aggregateSHA256 = "aggregate_sha256"
        case artifactMode = "artifact_mode"
        case name
        case requires
    }
}

struct KehtoIndex: Decodable {
    let applications: [KehtoApplication]
    let classification: String
    let digest: String
    let schema: Int
    let source: KehtoSource
}

struct KehtoApplication: Decodable {
    let gitTree: String
    let name: String
    let requires: [String]

    private enum CodingKeys: String, CodingKey {
        case gitTree = "git_tree"
        case name
        case requires
    }
}

struct KehtoSource: Decodable {
    let commit: String
    let repository: String
}

extension String {
    var displayCatalogTitle: String {
        split(separator: "-")
            .map { word in
                guard let first = word.first else {
                    return ""
                }
                return first.uppercased() + word.dropFirst()
            }
            .joined(separator: " ")
    }
}
