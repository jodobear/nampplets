import Foundation

extension RuntimeWorkbenchCatalogClient {
    static func loadRecords(bundle: Bundle) throws -> [CatalogRecord] {
        let decoder = JSONDecoder()
        let published = try decoder.decode(
            PublishedIndex.self,
            from: bundledIndexData(named: "published", bundle: bundle)
        )
        let reference = try decoder.decode(
            ReferenceIndex.self,
            from: bundledIndexData(named: "reference", bundle: bundle)
        )
        let kehto = try decoder.decode(
            KehtoIndex.self,
            from: bundledIndexData(named: "kehto", bundle: bundle)
        )

        guard published.schema == 1,
              published.classification == "published-immutable-artifacts",
              published.digest == publishedDigest,
              reference.schema == 1,
              reference.classification == "runtime-reference-fixtures",
              reference.digest == referenceDigest,
              kehto.schema == 1,
              kehto.classification == "kehto-source-corpus",
              kehto.digest == kehtoDigest,
              kehto.source.commit == kehtoCommit,
              kehto.source.repository == kehtoRepository
        else {
            throw CatalogResourceError.unexpectedBaseline
        }

        var result = try published.fixtures.map(publishedRecord)
        result.append(contentsOf: try reference.fixtures.map(referenceRecord))
        result.append(contentsOf: try kehto.applications.map(kehtoRecord))
        return result
    }

    private static let publishedDigest =
        "4bbd1218609000deaa273ef43c232211a90515c481dbd1929c40536c1e44e466"
    private static let referenceDigest =
        "5013983282a03741305b2f9740e2268ea6c038843b6e2214b0f34cbd611fd70a"
    private static let kehtoDigest =
        "225f96bc50c950260ecbdce14608fd2e82790acad64dc1cd2e835db5e1fc92a3"
    private static let kehtoCommit =
        "62241de0b4526ba4fdc8a7b3c766c2499d3ae24d"
    private static let kehtoRepository = "jodobear/kehto-web"

    private static func publishedRecord(
        fixture: PublishedFixture
    ) throws -> CatalogRecord {
        guard fixture.artifactMode == "single-file",
              fixture.coordinate.kind == 35_129,
              !fixture.name.isEmpty,
              !fixture.coordinate.author.isEmpty,
              !fixture.coordinate.dTag.isEmpty,
              !fixture.aggregateSHA256.isEmpty,
              !fixture.eventID.isEmpty,
              let eventFile = fixture.files.first(where: {
                  $0.path == "event.json"
              }),
              let indexFile = fixture.files.first(where: {
                  $0.artifactPath == "/index.html"
              })
        else {
            throw CatalogResourceError.unexpectedPublishedFixture
        }

        let title = fixture.name.displayCatalogTitle
        let coordinate =
            "\(fixture.coordinate.kind):\(fixture.coordinate.author):"
            + fixture.coordinate.dTag
        guard let entry = CatalogEntry(
            id: "published:\(fixture.eventID)",
            title: title,
            summary: "Published signed artifact that passes the shell-only legacy "
                + "host baseline; the complete provider journey is not ratified.",
            publisher: CatalogPublisher(
                displayName: nil,
                publicKey: fixture.coordinate.author
            ),
            coordinate: coordinate,
            compatibility: .incompatible(
                reason: "Current compatibility.lock advertises no macOS NAP "
                    + "domains. The pinned pass proves graceful shell-only boot, "
                    + "not the required identity, inc, and outbox journey."
            )
        ),
            let review = CatalogInstallReview(
                id: "published:\(fixture.eventID):\(fixture.aggregateSHA256)",
                title: title,
                publisher: entry.publisher,
                coordinate: coordinate,
                exactAggregateHash: fixture.aggregateSHA256,
                sources: [
                    CatalogSourceProvenance(
                        id: "published-index",
                        kind: .approvedCatalog,
                        source: "published/index.json",
                        evidence: "published-immutable-artifacts · corpus digest "
                            + publishedDigest
                    ),
                    CatalogSourceProvenance(
                        id: "manifest-event",
                        kind: .manifestEvent,
                        source: "kind 35129 event \(fixture.eventID)",
                        evidence: "Publisher \(fixture.coordinate.author) · "
                            + "d=\(fixture.coordinate.dTag) · event.json "
                            + "SHA-256 \(eventFile.sha256) · "
                            + "\(eventFile.bytes) bytes"
                    ),
                    CatalogSourceProvenance(
                        id: "artifact-index",
                        kind: .verifiedArtifactIndex,
                        source: "/index.html",
                        evidence: "SHA-256 \(indexFile.sha256) · "
                            + "\(indexFile.bytes) bytes · aggregate "
                            + fixture.aggregateSHA256
                    ),
                    CatalogSourceProvenance(
                        id: "artifact-sources",
                        kind: .artifact,
                        source: "Signed manifest server tags",
                        evidence: "Exact server tags preserved by the bundled "
                            + "signed manifest; no fetch is performed here."
                    ),
                ],
                requiredDomains: ["identity", "inc", "outbox"],
                optionalDomains: ["resource", "theme", "link"],
                platformCompatibility: [
                    CatalogPlatformCompatibility(
                        id: "macos",
                        platform: "macOS",
                        status: .incompatible,
                        detail: "compatibility.lock advertises no macOS NAP "
                            + "domains. The pinned legacy-host report observes an "
                            + "exact-byte shell boot and visible capability "
                            + "absence, not the complete provider journey."
                    ),
                    CatalogPlatformCompatibility(
                        id: "ios",
                        platform: "iOS",
                        status: .unavailable,
                        detail: "Not run in the pinned reports; "
                            + "compatibility.lock advertises no iOS domains."
                    ),
                    CatalogPlatformCompatibility(
                        id: "android",
                        platform: "Android",
                        status: .unavailable,
                        detail: "Not run in the pinned reports; "
                            + "compatibility.lock advertises no Android domains."
                    ),
                ],
                warnings: [
                    CatalogWarning(
                        id: "baseline-unratified",
                        severity: .caution,
                        message: "The native-runtime-compat-v2 baseline is "
                            + "unratified and its overall legacy-host report is "
                            + "incomplete."
                    ),
                    CatalogWarning(
                        id: "provider-journey-unproven",
                        severity: .caution,
                        message: "The pass proves secure shell boot and graceful "
                            + "capability absence, not identity, inbox, outbox, "
                            + "resource, theme, or link behavior end to end."
                    ),
                    CatalogWarning(
                        id: "install-boundary-unavailable",
                        severity: .blocking,
                        message: "The Workbench has not connected the Rust "
                            + "resolver/install-only boundary. This review cannot "
                            + "install, launch, or grant the build."
                    ),
                ],
                updateRelationship: .firstInstall,
                canInstall: false
            )
        else {
            throw CatalogResourceError.outsideUILimits
        }

        return CatalogRecord(
            entry: entry,
            review: review,
            reviewIssue: nil,
            searchTerms: [
                fixture.name,
                fixture.coordinate.author,
                fixture.eventID,
                fixture.aggregateSHA256,
                "identity inc outbox resource theme link",
            ]
        )
    }

    private static func referenceRecord(
        fixture: ReferenceFixture
    ) throws -> CatalogRecord {
        let missing = fixture.requires.sorted()
        let reason: String
        if fixture.name == "missing-domain" {
            reason = "Incompatible: requires ble, which compatibility.lock "
                + "does not advertise on macOS."
        } else if fixture.name == "external-assets" {
            reason = "Unavailable: the pinned report did not run the external "
                + "asset module because its harness cannot register the native "
                + "artifact URL scheme."
        } else {
            reason = "Unavailable: reference-only compatibility fixture; it is "
                + "not a published signed install."
        }
        let compatibility: CatalogCompatibilitySummary =
            fixture.name == "missing-domain"
            ? .incompatible(reason: reason)
            : .unknown(reason: reason)
        let coordinate =
            "unavailable:reference/\(fixture.name)#\(fixture.aggregateSHA256)"
        guard let entry = CatalogEntry(
            id: "reference:\(fixture.name):\(fixture.aggregateSHA256)",
            title: fixture.name.displayCatalogTitle,
            summary: reason,
            publisher: CatalogPublisher(
                displayName: "Pinned conformance corpus",
                publicKey: "Unavailable — no signed publisher"
            ),
            coordinate: coordinate,
            compatibility: compatibility
        ) else {
            throw CatalogResourceError.outsideUILimits
        }
        return CatalogRecord(
            entry: entry,
            review: nil,
            reviewIssue: CatalogIssue(
                title: "Reference fixture unavailable",
                message: reason + " Exact aggregate: \(fixture.aggregateSHA256)."
            ),
            searchTerms: missing + [fixture.name, fixture.artifactMode, reason]
        )
    }

    private static func kehtoRecord(
        application: KehtoApplication
    ) throws -> CatalogRecord {
        let domains = application.requires.sorted()
        let domainText = domains.isEmpty ? "no declared domains" : domains.joined(
            separator: ", "
        )
        let reason = "Built, not run: the pinned macOS report preflight-blocked "
            + "this source application; required domains: \(domainText)."
        let coordinate =
            "unavailable:kehto/\(application.name)@\(application.gitTree)"
        guard let entry = CatalogEntry(
            id: "kehto:\(application.name):\(application.gitTree)",
            title: application.name.displayCatalogTitle,
            summary: reason,
            publisher: CatalogPublisher(
                displayName: "\(kehtoRepository) @ \(kehtoCommit.prefix(12))",
                publicKey: "Unavailable — source corpus is not a signed manifest"
            ),
            coordinate: coordinate,
            compatibility: .incompatible(reason: reason)
        ) else {
            throw CatalogResourceError.outsideUILimits
        }
        return CatalogRecord(
            entry: entry,
            review: nil,
            reviewIssue: CatalogIssue(
                title: "Built source is not installable",
                message: reason + " Exact source tree: \(application.gitTree)."
            ),
            searchTerms: domains
                + [application.name, application.gitTree, "built not run"]
        )
    }
}
