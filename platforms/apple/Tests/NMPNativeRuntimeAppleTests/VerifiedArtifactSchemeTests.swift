import Foundation
import Testing
import WebKit
import XCTest
@testable import NMPNativeRuntimeApple

@MainActor
@Suite("Verified artifact URL scheme")
struct VerifiedArtifactSchemeContractTests {
    private let session = "11111111-1111-4111-8111-111111111111"
    private let digest = String(repeating: "a", count: 64)

    @Test("exact session and path are the only lookup authority")
    func exactAuthorityAndPath() throws {
        let handler = makeHandler(paths: [
            "/index.html": Data("index".utf8),
            "/Scripts/App.JS": Data("script".utf8)
        ])

        #expect(
            try handler.resolve(URL(string: "nmp-artifact://\(session)/index.html"))
                .bytes == Data("index".utf8)
        )
        #expect(throws: VerifiedArtifactSchemeFailure.wrongSession) {
            try handler.resolve(
                URL(string: "nmp-artifact://22222222-2222-4222-8222-222222222222/index.html")
            )
        }
        #expect(throws: VerifiedArtifactSchemeFailure.unknownPath) {
            try handler.resolve(URL(string: "nmp-artifact://\(session)/scripts/app.js"))
        }
        #expect(
            try handler.resolve(URL(string: "nmp-artifact://\(session)/Scripts/App.JS"))
                .bytes == Data("script".utf8)
        )
    }

    @Test("encoded Unicode traversal and ambiguous URL forms fail closed")
    func rejectsNonCanonicalURLs() throws {
        let handler = makeHandler(paths: ["/index.html": Data("index".utf8)])
        let invalidURLs = [
            "nmp-artifact://\(session)/scripts%2Fboot.js",
            "nmp-artifact://\(session)/caf%C3%A9.js",
            "nmp-artifact://\(session)/scripts/../index.html",
            "nmp-artifact://\(session)//index.html",
            "nmp-artifact://\(session)/index.html?variant=1",
            "nmp-artifact://\(session)/index.html#fragment",
            "nmp-artifact://user@\(session)/index.html",
            "nmp-artifact://\(session):443/index.html"
        ]

        for raw in invalidURLs {
            #expect(throws: (any Error).self, "must reject \(raw)") {
                try handler.resolve(URL(string: raw))
            }
        }
    }

    @Test("MIME mapping is deterministic and unknown types never sniff")
    func deterministicMIME() {
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/index.HTML") == "text/html; charset=utf-8")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/app.mjs") == "text/javascript; charset=utf-8")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/style.css") == "text/css; charset=utf-8")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/image.svg") == "image/svg+xml")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/font.woff2") == "font/woff2")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/sound.ogg") == "audio/ogg")
        #expect(VerifiedArtifactSchemeHandler.mimeType(for: "/unknown.bin") == "application/octet-stream")
    }

    @Test("per-file and cumulative response limits refuse observably")
    func finiteLimits() throws {
        let files = [
            "/one.bin": Data(repeating: 1, count: 4),
            "/two.bin": Data(repeating: 2, count: 4)
        ]
        let perFile = makeHandler(
            paths: files,
            limits: .init(
                maximumConcurrentResponses: 1,
                maximumFileBytes: 3,
                maximumSessionBytes: 6
            )
        )
        #expect(
            throws: VerifiedArtifactSchemeFailure.responseTooLarge(actual: 4, maximum: 3)
        ) {
            try perFile.resolve(URL(string: "nmp-artifact://\(session)/one.bin"))
        }

        let cumulative = makeHandler(
            paths: files,
            limits: .init(
                maximumConcurrentResponses: 1,
                maximumFileBytes: 4,
                maximumSessionBytes: 6
            )
        )
        _ = try cumulative.resolve(URL(string: "nmp-artifact://\(session)/one.bin"))
        #expect(
            throws: VerifiedArtifactSchemeFailure.sessionLimitExceeded(actual: 8, maximum: 6)
        ) {
            try cumulative.resolve(URL(string: "nmp-artifact://\(session)/two.bin"))
        }
    }

    @Test("teardown is terminal and idempotent")
    func teardown() {
        let handler = makeHandler(paths: ["/index.html": Data("index".utf8)])
        handler.teardown()
        handler.teardown()
        #expect(throws: VerifiedArtifactSchemeFailure.stopped) {
            try handler.resolve(URL(string: "nmp-artifact://\(session)/index.html"))
        }
    }

    @Test("the Swift projection rejects a lying reader contract")
    func readerContractViolation() {
        let handler = VerifiedArtifactSchemeHandler(
            sessionID: session,
            reader: LyingReader()
        )
        #expect(throws: VerifiedArtifactSchemeFailure.readerContractViolation) {
            try handler.resolve(URL(string: "nmp-artifact://\(session)/index.html"))
        }
    }

    private func makeHandler(
        paths: [String: Data],
        limits: VerifiedArtifactSchemeLimits = .production
    ) -> VerifiedArtifactSchemeHandler {
        VerifiedArtifactSchemeHandler(
            sessionID: session,
            reader: InMemoryVerifiedArtifactReader(
                files: paths.map {
                    SealedArtifactBytes(
                        logicalPath: $0.key,
                        sha256: digest,
                        bytes: $0.value
                    )
                }
            ),
            limits: limits
        )
    }
}

@MainActor
final class VerifiedArtifactWebKitIntegrationTests: XCTestCase {
    func testExternalScriptStyleAndImageLoadOnlyFromSessionScheme() async throws {
        let paths = [
            "/index.html",
            "/styles/site.css",
            "/scripts/boot.js",
            "/images/verified.svg"
        ]
        let files = try paths.map { logicalPath -> SealedArtifactBytes in
            let url = try XCTUnwrap(TrustedShellResources.externalFixtureURL(logicalPath))
            return SealedArtifactBytes(
                logicalPath: logicalPath,
                sha256: String(repeating: "b", count: 64),
                bytes: try Data(contentsOf: url)
            )
        }
        let artifact = NappletArtifact(
            title: "Verified external assets",
            reader: InMemoryVerifiedArtifactReader(files: files)
        )
        let externalAssetsLoaded = expectation(
            description: "verified external script, style, and image all loaded"
        )
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .request(type: "shell.ping") {
                externalAssetsLoaded.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [externalAssetsLoaded], timeout: 10)
        let result = try await webView.callAsyncJavaScript(
            """
            const frame = document.getElementById("napplet-frame");
            return {
              frameSourceIsSrcdoc: frame.getAttribute("src") === null,
              frameSandbox: frame.getAttribute("sandbox")
            };
            """,
            arguments: [:],
            in: nil,
            contentWorld: .page
        )
        let state = try XCTUnwrap(result as? [String: Any])
        XCTAssertEqual(state["frameSourceIsSrcdoc"] as? Bool, true)
        XCTAssertEqual(state["frameSandbox"] as? String, "allow-scripts")
    }
}

private struct LyingReader: VerifiedArtifactByteReader {
    func readSealed(logicalPath: String) throws -> SealedArtifactBytes? {
        SealedArtifactBytes(
            logicalPath: "/different.html",
            sha256: String(repeating: "c", count: 64),
            bytes: Data()
        )
    }
}
