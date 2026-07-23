import Foundation
import NMPNativeRuntime

public enum RuntimeNappletOpenError: Error, LocalizedError, Equatable {
    case invalidStorageRoot
    case artifactRefused(code: String, detail: String)
    case launchRefused(detail: String)
    case observerRefused(code: String, detail: String)

    public var errorDescription: String? {
        switch self {
        case .invalidStorageRoot:
            "The native runtime storage directory is unavailable."
        case let .artifactRefused(code, detail):
            "Artifact verification was refused (\(code)): \(detail)"
        case let .launchRefused(detail):
            "The native runtime refused to launch the artifact: \(detail)"
        case let .observerRefused(code, detail):
            "Runtime observation was refused (\(code)): \(detail)"
        }
    }
}

/// Immutable bytes supplied to Rust's signed-manifest resolver.
///
/// The callback does not decide whether a URL, digest, redirect, size, or
/// response is acceptable. It reports only the bytes and source selected by
/// the Rust-owned request; Rust revalidates every fact before sealing them.
private final class BundledArtifactSource: ArtifactSource, @unchecked Sendable {
    private let bytesByDigest: [String: Data]

    init(bytesByDigest: [String: Data]) {
        self.bytesByDigest = bytesByDigest
    }

    func fetch(request: ArtifactFetchRequest) -> ArtifactFetchResponse {
        guard let bytes = bytesByDigest[request.expectedSha256] else {
            return .refused(reason: "No bundled bytes match the requested digest")
        }
        guard let sourceURL = request.candidateUrls.first else {
            return .refused(reason: "The verified manifest has no candidate source")
        }
        return .body(sourceUrl: sourceURL, httpStatus: 200, bytes: bytes)
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
final class RustRuntimeNappletSession:
    RuntimeObserver,
    TrustedNappletRuntimeSession,
    @unchecked Sendable
{
    let sessionID: UInt64

    private let controller: RuntimeController
    private let maximumReadBytes: UInt64
    private let lock = NSLock()
    private var observation: RuntimeObservation?
    private var responseSink: (@Sendable (Data) -> Void)?
    private var isStopped = false

    init(
        controller: RuntimeController,
        sessionID: UInt64,
        maximumReadBytes: UInt64
    ) {
        self.controller = controller
        self.sessionID = sessionID
        self.maximumReadBytes = maximumReadBytes
    }

    func startObservation() throws {
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

    func readSealed(logicalPath: String) throws -> SealedArtifactBytes? {
        switch controller.readVerified(
            sessionId: sessionID,
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
        controller.mappedEnvelope(sessionId: sessionID, bytes: bytes)
    }

    func update(frame: RuntimeObservationFrame) {
        lock.lock()
        let sink = responseSink
        let stopped = isStopped
        lock.unlock()
        guard !stopped, let sink else { return }

        for event in frame.events
        where event.kind == "envelope-handled"
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
        let observation = observation
        self.observation = nil
        lock.unlock()

        observation?.stop()
        controller.stop(sessionId: sessionID)
        controller.close()
    }

    func crash(reason: String) {
        lock.lock()
        let stopped = isStopped
        lock.unlock()
        guard !stopped else { return }
        controller.crash(sessionId: sessionID, reason: reason)
    }

    deinit {
        stop()
    }
}

extension NappletArtifact {
    /// Verifies a signed named manifest, installs its exact build, grants only
    /// the explicitly supplied domains, and launches one Rust-owned session.
    ///
    /// `blobsBySHA256` is a bounded native capability result. It never becomes
    /// executable until Rust verifies the event signature, coordinate, sources,
    /// per-file digests, aggregate digest, and artifact limits.
    public static func openSignedNamed(
        title: String,
        eventJSON: Data,
        author: String,
        dTag: String,
        blobsBySHA256: [String: Data],
        grantDomains: [String],
        storageRoot: URL
    ) throws -> Self {
        do {
            try FileManager.default.createDirectory(
                at: storageRoot,
                withIntermediateDirectories: true
            )
        } catch {
            throw RuntimeNappletOpenError.invalidStorageRoot
        }

        let maximumReadBytes: UInt64 = 8 * 1_024 * 1_024
        let controller = try RuntimeController.open(
            config: RuntimeConfig(
                runtimeStorePath: storageRoot
                    .appendingPathComponent("runtime.sqlite3")
                    .path,
                nmpStorePath: nil,
                artifactCachePath: storageRoot
                    .appendingPathComponent("artifacts", isDirectory: true)
                    .path,
                indexerRelays: [],
                appRelays: [],
                fallbackRelays: [],
                allowedLocalRelayHosts: [],
                maximumNmpRelays: 8,
                maximumBridgeWorkers: 4,
                maximumObservers: 2,
                maximumBoundaryEvents: 32,
                maximumConfigItems: 32,
                maximumConfigStringBytes: 16_384,
                maximumManifestBytes: 262_144,
                maximumArtifactFiles: 64,
                maximumArtifactFileBytes: maximumReadBytes,
                maximumArtifactTotalBytes: 32 * 1_024 * 1_024,
                maximumVerifiedReadBytes: maximumReadBytes,
                maximumBlobSources: 8
            ),
            artifactSource: BundledArtifactSource(
                bytesByDigest: blobsBySHA256
            )
        )

        let verification = controller.verifyArtifact(
            eventJson: eventJSON,
            coordinate: .named(author: author, dTag: dTag)
        )
        guard let artifact = verification.artifact else {
            controller.close()
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
        controller.launch(
            artifact: artifact,
            profile: .legacy
        )

        let aggregateHash = artifact.aggregateHash()
        guard let launched = controller.snapshot().sessions.first(where: {
            $0.author == author
                && $0.dTag == dTag
                && $0.aggregateHash == aggregateHash
                && $0.state == "running"
        }) else {
            let snapshot = controller.snapshot()
            let detail = snapshot.recentErrors.last?.detail
                ?? snapshot.boundaryRefusals.last?.detail
                ?? "No running session was created"
            controller.close()
            throw RuntimeNappletOpenError.launchRefused(detail: detail)
        }

        let runtime = RustRuntimeNappletSession(
            controller: controller,
            sessionID: launched.id,
            maximumReadBytes: maximumReadBytes
        )
        do {
            try runtime.startObservation()
        } catch {
            runtime.stop()
            throw error
        }
        return NappletArtifact(
            title: title,
            reader: runtime,
            runtimeSession: runtime,
            negotiatedDomains: launched.domains
        )
    }
}
