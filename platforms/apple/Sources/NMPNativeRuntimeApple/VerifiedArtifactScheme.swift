import Foundation
import WebKit

/// The narrow adapter boundary that a generated Rust FFI artifact handle will
/// satisfy. It is intentionally internal: application code cannot manufacture
/// an executable artifact by passing arbitrary HTML or filesystem locations.
///
/// Implementations return only bytes that Rust has already verified and sealed
/// under an exact logical path. Swift does not verify manifests, follow
/// redirects, fetch remote content, or derive native paths.
protocol VerifiedArtifactByteReader: Sendable {
    func readSealed(logicalPath: String) throws -> SealedArtifactBytes?
}

struct SealedArtifactBytes: Sendable, Equatable {
    let logicalPath: String
    let sha256: String
    let bytes: Data
}

enum VerifiedArtifactReaderError: Error, Equatable {
    case unavailable
}

struct InMemoryVerifiedArtifactReader: VerifiedArtifactByteReader {
    private let files: [String: SealedArtifactBytes]

    init(files: [SealedArtifactBytes]) {
        self.files = Dictionary(
            uniqueKeysWithValues: files.map { ($0.logicalPath, $0) }
        )
    }

    func readSealed(logicalPath: String) throws -> SealedArtifactBytes? {
        files[logicalPath]
    }
}

struct VerifiedArtifactSchemeLimits: Sendable, Equatable {
    let maximumConcurrentResponses: Int
    let maximumFileBytes: Int
    let maximumSessionBytes: Int

    static let production = Self(
        maximumConcurrentResponses: 16,
        maximumFileBytes: 8 * 1024 * 1024,
        maximumSessionBytes: 32 * 1024 * 1024
    )

    var isValid: Bool {
        maximumConcurrentResponses > 0
            && maximumFileBytes > 0
            && maximumSessionBytes >= maximumFileBytes
    }
}

enum VerifiedArtifactSchemeFailure: Error, Equatable {
    case stopped
    case malformedURL
    case wrongSession
    case invalidPath
    case unknownPath
    case readerFailure
    case readerContractViolation
    case responseTooLarge(actual: Int, maximum: Int)
    case sessionLimitExceeded(actual: Int, maximum: Int)
    case concurrencyLimitExceeded(maximum: Int)
}

struct VerifiedArtifactSchemeResponse: Sendable, Equatable {
    let logicalPath: String
    let digest: String
    let bytes: Data
    let mimeType: String
}

/// A session-owned, non-networked projection of verified artifact bytes.
///
/// URL authority is the native-created session token. Requests have no remote
/// fallback and are never converted into filesystem URLs.
final class VerifiedArtifactSchemeHandler: NSObject, WKURLSchemeHandler, @unchecked Sendable {
    static let scheme = "nmp-artifact"

    let sessionID: String
    let baseURL: URL

    private let reader: any VerifiedArtifactByteReader
    private let limits: VerifiedArtifactSchemeLimits
    private let lock = NSLock()
    private var stopped = false
    private var activeTasks: Set<ObjectIdentifier> = []
    private var deliveredBytes = 0

    init(
        sessionID: String,
        reader: any VerifiedArtifactByteReader,
        limits: VerifiedArtifactSchemeLimits = .production
    ) {
        precondition(limits.isValid, "verified artifact scheme limits must be finite")
        precondition(Self.isCanonicalSessionID(sessionID), "session ID must be canonical")
        self.sessionID = sessionID
        self.reader = reader
        self.limits = limits
        self.baseURL = URL(string: "\(Self.scheme)://\(sessionID)/")!
        super.init()
    }

    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        let taskID = ObjectIdentifier(urlSchemeTask as AnyObject)
        do {
            try begin(taskID)
            let response = try resolve(urlSchemeTask.request.url)
            guard finishIfActive(taskID) else { return }
            let headers = [
                "Content-Type": response.mimeType,
                "Content-Length": String(response.bytes.count),
                "Cache-Control": "private, immutable",
                "X-Content-Type-Options": "nosniff"
            ]
            guard let url = urlSchemeTask.request.url,
                  let urlResponse = HTTPURLResponse(
                      url: url,
                      statusCode: 200,
                      httpVersion: "HTTP/1.1",
                      headerFields: headers
                  )
            else {
                urlSchemeTask.didFailWithError(
                    VerifiedArtifactSchemeFailure.readerContractViolation
                )
                return
            }
            urlSchemeTask.didReceive(urlResponse)
            urlSchemeTask.didReceive(response.bytes)
            urlSchemeTask.didFinish()
        } catch {
            // A task canceled by WebKit or removed by teardown must receive no
            // later callback, even if a synchronous FFI read completed after
            // cancellation.
            if removeIfActive(taskID) {
                urlSchemeTask.didFailWithError(error)
            }
        }
    }

    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) {
        _ = removeIfActive(ObjectIdentifier(urlSchemeTask as AnyObject))
    }

    func teardown() {
        lock.lock()
        stopped = true
        activeTasks.removeAll(keepingCapacity: false)
        lock.unlock()
    }

    /// Internal for deterministic contract tests. Production delivery enters
    /// through `WKURLSchemeHandler.start`.
    func resolve(_ url: URL?) throws -> VerifiedArtifactSchemeResponse {
        lock.lock()
        let isStopped = stopped
        lock.unlock()
        guard !isStopped else {
            throw VerifiedArtifactSchemeFailure.stopped
        }

        let logicalPath = try canonicalLogicalPath(from: url)
        let sealed: SealedArtifactBytes
        do {
            guard let found = try reader.readSealed(logicalPath: logicalPath) else {
                throw VerifiedArtifactSchemeFailure.unknownPath
            }
            sealed = found
        } catch let failure as VerifiedArtifactSchemeFailure {
            throw failure
        } catch {
            throw VerifiedArtifactSchemeFailure.readerFailure
        }

        guard sealed.logicalPath == logicalPath,
              Self.isLowercaseSHA256(sealed.sha256)
        else {
            throw VerifiedArtifactSchemeFailure.readerContractViolation
        }
        guard sealed.bytes.count <= limits.maximumFileBytes else {
            throw VerifiedArtifactSchemeFailure.responseTooLarge(
                actual: sealed.bytes.count,
                maximum: limits.maximumFileBytes
            )
        }

        lock.lock()
        defer { lock.unlock() }
        guard !stopped else {
            throw VerifiedArtifactSchemeFailure.stopped
        }
        let newTotal = deliveredBytes.addingReportingOverflow(sealed.bytes.count)
        guard !newTotal.overflow, newTotal.partialValue <= limits.maximumSessionBytes else {
            throw VerifiedArtifactSchemeFailure.sessionLimitExceeded(
                actual: newTotal.overflow ? .max : newTotal.partialValue,
                maximum: limits.maximumSessionBytes
            )
        }
        deliveredBytes = newTotal.partialValue
        return VerifiedArtifactSchemeResponse(
            logicalPath: logicalPath,
            digest: sealed.sha256,
            bytes: sealed.bytes,
            mimeType: Self.mimeType(for: logicalPath)
        )
    }

    private func begin(_ taskID: ObjectIdentifier) throws {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else {
            throw VerifiedArtifactSchemeFailure.stopped
        }
        guard activeTasks.count < limits.maximumConcurrentResponses else {
            throw VerifiedArtifactSchemeFailure.concurrencyLimitExceeded(
                maximum: limits.maximumConcurrentResponses
            )
        }
        activeTasks.insert(taskID)
    }

    private func finishIfActive(_ taskID: ObjectIdentifier) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !stopped else {
            activeTasks.remove(taskID)
            return false
        }
        return activeTasks.remove(taskID) != nil
    }

    @discardableResult
    private func removeIfActive(_ taskID: ObjectIdentifier) -> Bool {
        lock.lock()
        let wasActive = activeTasks.remove(taskID) != nil
        lock.unlock()
        return wasActive
    }

    private func canonicalLogicalPath(from url: URL?) throws -> String {
        guard let url,
              url.scheme == Self.scheme,
              url.user == nil,
              url.password == nil,
              url.port == nil,
              url.query == nil,
              url.fragment == nil
        else {
            throw VerifiedArtifactSchemeFailure.malformedURL
        }
        guard url.host == sessionID else {
            throw VerifiedArtifactSchemeFailure.wrongSession
        }
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false) else {
            throw VerifiedArtifactSchemeFailure.malformedURL
        }
        let path = components.percentEncodedPath
        guard !path.contains("%"),
              path.utf8.allSatisfy({ $0 < 0x80 && !$0.isASCIIControl }),
              path.first == "/",
              path.count > 1,
              !path.contains("\\"),
              path.split(separator: "/", omittingEmptySubsequences: false)
                  .dropFirst()
                  .allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." })
        else {
            throw VerifiedArtifactSchemeFailure.invalidPath
        }
        return path
    }

    static func mimeType(for logicalPath: String) -> String {
        switch (logicalPath as NSString).pathExtension.lowercased() {
        case "html", "htm": "text/html; charset=utf-8"
        case "css": "text/css; charset=utf-8"
        case "js", "mjs": "text/javascript; charset=utf-8"
        case "json", "map": "application/json; charset=utf-8"
        case "svg": "image/svg+xml"
        case "png": "image/png"
        case "jpg", "jpeg": "image/jpeg"
        case "gif": "image/gif"
        case "webp": "image/webp"
        case "avif": "image/avif"
        case "ico": "image/x-icon"
        case "woff": "font/woff"
        case "woff2": "font/woff2"
        case "ttf": "font/ttf"
        case "otf": "font/otf"
        case "mp3": "audio/mpeg"
        case "m4a": "audio/mp4"
        case "ogg": "audio/ogg"
        case "wav": "audio/wav"
        case "mp4", "m4v": "video/mp4"
        case "webm": "video/webm"
        case "wasm": "application/wasm"
        case "txt": "text/plain; charset=utf-8"
        default: "application/octet-stream"
        }
    }

    private static func isCanonicalSessionID(_ value: String) -> Bool {
        value.count == 36
            && value == value.lowercased()
            && UUID(uuidString: value) != nil
    }

    private static func isLowercaseSHA256(_ value: String) -> Bool {
        value.count == 64
            && value.utf8.allSatisfy {
                ($0 >= 0x30 && $0 <= 0x39) || ($0 >= 0x61 && $0 <= 0x66)
            }
    }
}

private extension UInt8 {
    var isASCIIControl: Bool {
        self < 0x20 || self == 0x7f
    }
}
