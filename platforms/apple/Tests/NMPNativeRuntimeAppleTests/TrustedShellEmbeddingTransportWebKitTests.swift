import Foundation
import WebKit
import XCTest
@testable import NMPNativeRuntimeApple
@MainActor
final class TrustedShellEmbeddingTransportWebKitTests: XCTestCase {
    func testChunkedOuterMountUsesRealWebKitStructuredCloneAndRetiresState()
        async throws
    {
        let shellURL = try XCTUnwrap(TrustedShellResources.shellURL)
        let embeddedURL = shellURL.deletingLastPathComponent()
            .appendingPathComponent("trusted-shell-embedded.html")
        let original = try String(contentsOf: embeddedURL, encoding: .utf8)
        let bootstrap = """
        if (window.parent !== window) {
          NMPTrustedShellEmbedding.createEmbeddingBridge(window);
        }
        """
        XCTAssertEqual(original.components(separatedBy: bootstrap).count, 2)
        let controlledBootstrap = """
        window.__createChunkTestBridge = function (policy, timeoutMs) {
          const copiedPolicy = JSON.parse(JSON.stringify(policy));
          window.__chunkTestBridge =
            NMPTrustedShellEmbedding.createEmbeddingBridge(window, {
              artifactPolicy: copiedPolicy,
              setTransferTimeout(callback, _milliseconds) {
                return window.setTimeout(callback, timeoutMs);
              },
              clearTransferTimeout(identifier) {
                window.clearTimeout(identifier);
              }
            });
          return true;
        };
        """
        let temporary = FileManager.default.temporaryDirectory
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(
            at: temporary,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: temporary) }
        let outerURL = temporary.appendingPathComponent("outer.html")
        try original.replacingOccurrences(
            of: bootstrap,
            with: controlledBootstrap
        ).write(to: outerURL, atomically: true, encoding: .utf8)
        let parentURL = temporary.appendingPathComponent("parent.html")
        try "<!doctype html><html><body></body></html>".write(
            to: parentURL,
            atomically: true,
            encoding: .utf8
        )
        let loaded = expectation(description: "parent WebKit document loaded")
        let navigation = ChunkTransportNavigationProbe(loaded)
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        let webView = WKWebView(
            frame: CGRect(x: 0, y: 0, width: 800, height: 600),
            configuration: configuration
        )
        webView.navigationDelegate = navigation
        webView.loadFileURL(parentURL, allowingReadAccessTo: temporary)
        await fulfillment(of: [loaded], timeout: 10)
        let result = try await webView.callAsyncJavaScript(
            Self.transportProof,
            arguments: ["outerURL": outerURL.absoluteString],
            in: nil,
            contentWorld: .page
        )
        let state = try XCTUnwrap(result as? [String: Any])
        XCTAssertEqual(state["chunkCountExceedsWindow"] as? Bool, true)
        XCTAssertEqual(state["splitUtf8Preserved"] as? Bool, true)
        XCTAssertEqual(state["exactMaterializedDigest"] as? Bool, true)
        XCTAssertEqual(state["commitResults"] as? Int, 1)
        XCTAssertEqual(state["srcdocAssignments"] as? Int, 1)
        XCTAssertEqual(state["timeoutExpired"] as? Bool, true)
        XCTAssertEqual(state["timeoutStateRetired"] as? Bool, true)
        XCTAssertEqual(state["pagehideStateRetired"] as? Bool, true)
        XCTAssertEqual(state["refusedSrcdocAssignments"] as? Int, 0)
        XCTAssertEqual(state["errors"] as? [String], [])
    }
    private static let transportProof = #"""
    const CHUNK = 256 * 1024;
    const encoder = new TextEncoder();
    const prefix = "\uFEFF<!doctype html><html><head></head><body><!--";
    const splitAt = CHUNK - 1;
    const filler = "a".repeat(splitAt - encoder.encode(prefix).byteLength);
    const artifact = prefix + filler + "€終" + "b".repeat(CHUNK * 256) +
      "--></body></html>";
    const artifactBytes = encoder.encode(artifact);
    const baseURL = "nmp-artifact://00000000-0000-4000-8000-000000000001/";
    const errors = [];
    async function sha256(text) {
      const bytes = encoder.encode(text);
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      return Array.from(new Uint8Array(digest))
        .map(byte => byte.toString(16).padStart(2, "0")).join("");
    }
    function waitFor(messages, predicate, label, timeoutMs = 30000) {
      return new Promise((resolve, reject) => {
        const started = performance.now();
        function inspect() {
          const match = messages.find(predicate);
          if (match) return resolve(match);
          if (performance.now() - started >= timeoutMs) {
            return reject(new Error("timeout:" + label));
          }
          setTimeout(inspect, 5);
        }
        inspect();
      });
    }
    async function makeOuter(name, policy, timeoutMs) {
      const frame = document.createElement("iframe");
      frame.id = name;
      const messages = [];
      const listener = event => {
        if (event.source === frame.contentWindow) messages.push(event.data);
      };
      window.addEventListener("message", listener);
      await new Promise((resolve, reject) => {
        frame.onload = resolve;
        frame.onerror = () => reject(new Error("outer-load:" + name));
        frame.src = outerURL;
        document.body.appendChild(frame);
      });
      const child = frame.contentWindow;
      child.addEventListener("error", event => errors.push(String(event.message)));
      child.addEventListener("unhandledrejection", event =>
        errors.push(String(event.reason)));
      child.__testSrcdocAssignments = 0;
      child.__testLastSrcdoc = null;
      const descriptor = child.Object.getOwnPropertyDescriptor(
        child.HTMLIFrameElement.prototype,
        "srcdoc"
      );
      child.Object.defineProperty(child.HTMLIFrameElement.prototype, "srcdoc", {
        configurable: descriptor.configurable,
        enumerable: descriptor.enumerable,
        get: descriptor.get,
        set(value) {
          child.__testSrcdocAssignments += 1;
          child.__testLastSrcdoc = value;
          descriptor.set.call(this, value);
        }
      });
      if (policy !== undefined) {
        child.__createChunkTestBridge(policy, timeoutMs);
        await waitFor(messages, message => message.type === "nmp.outer.ready", name);
      }
      return { child, frame, listener, messages };
    }
    function configuration(session, surfaceId, artifactDigest) {
      return {
        session,
        artifactBytes: artifactBytes.byteLength,
        artifactBaseURL: baseURL,
        domains: ["shell"],
        title: "Chunk transport WebKit proof",
        binding: {
          manifestAuthor: "a".repeat(64),
          dTag: "chunk-webkit",
          aggregateHash: "b".repeat(64),
          artifactDigest,
          surface: surfaceId,
          session
        }
      };
    }
    function sendBegin(outer, session, surfaceId, artifactDigest, requestId) {
      outer.child.postMessage({
        type: "nmp.outer.mount.begin",
        requestId,
        surfaceId,
        configuration: configuration(session, surfaceId, artifactDigest)
      }, "*");
    }
    function retireOuter(outer) {
      outer.child.dispatchEvent(new outer.child.PageTransitionEvent("pagehide"));
      window.removeEventListener("message", outer.listener);
      outer.frame.remove();
    }
    const policyFrame = await makeOuter("policy-frame", undefined, 60000);
    const materialized = policyFrame.child.NMPTrustedShellPrimitives.materialize(
      artifact,
      baseURL,
      ["shell"]
    );
    const artifactDigest = await sha256(artifact);
    const materializedDigest = await sha256(materialized);
    retireOuter(policyFrame);
    const policy = {
      maximumArtifactHTMLBytes: 120 * 1024 * 1024,
      exactArtifactHTMLBytes: artifactBytes.byteLength,
      maximumMaterializedHTMLBytes: 120 * 1024 * 1024,
      exactMaterializedHTMLBytes: encoder.encode(materialized).byteLength,
      artifactDigest,
      materializedDigest,
      manifestAuthor: "a".repeat(64),
      dTag: "chunk-webkit",
      aggregateHash: "b".repeat(64),
      exclusive: true
    };
    const positive = await makeOuter("positive", policy, 60000);
    sendBegin(positive, "positive-session", "surface-positive", artifactDigest,
      "positive-begin");
    await waitFor(positive.messages, message =>
      message.type === "nmp.outer.mount.begin.result" &&
      message.requestId === "positive-begin" && message.ok === true,
    "positive-begin");
    const transferId = "positive-begin";
    let chunkCount = 0;
    for (let offset = 0; offset < artifactBytes.byteLength; offset += CHUNK) {
      const chunk = artifactBytes.slice(offset, offset + CHUNK).buffer;
      positive.child.postMessage({
        type: "nmp.outer.mount.chunk",
        requestId: "positive-chunk-" + offset,
        surfaceId: "surface-positive",
        session: "positive-session",
        transferId,
        offset,
        bytes: chunk
      }, "*", [chunk]);
      chunkCount += 1;
      if (chunkCount % 16 === 0) await new Promise(resolve => setTimeout(resolve, 0));
    }
    positive.child.postMessage({
      type: "nmp.outer.mount.commit",
      requestId: "positive-commit",
      surfaceId: "surface-positive",
      session: "positive-session",
      transferId
    }, "*");
    await waitFor(positive.messages, message =>
      message.type === "nmp.outer.mount.commit.result" &&
      message.requestId === "positive-commit" && message.ok === true,
    "positive-commit", 60000);
    await waitFor(positive.messages, message =>
      message.type === "nmp.outer.surface.ready" &&
      message.surfaceId === "surface-positive",
    "positive-ready", 60000);
    const positiveDigest = await sha256(positive.child.__testLastSrcdoc);
    const commitResults = positive.messages.filter(message =>
      message.type === "nmp.outer.mount.commit.result").length;
    const srcdocAssignments = positive.child.__testSrcdocAssignments;
    retireOuter(positive);
    const timeout = await makeOuter("timeout", policy, 20);
    sendBegin(timeout, "timeout-session", "surface-timeout", artifactDigest,
      "timeout-begin");
    await waitFor(timeout.messages, message =>
      message.type === "nmp.outer.mount.begin.result" && message.ok === true,
    "timeout-begin");
    const expired = await waitFor(timeout.messages, message =>
      message.type === "nmp.outer.mount.begin.result" &&
      message.error === "transfer-expired",
    "timeout-expiry");
    const timeoutCounts = timeout.child.__chunkTestBridge.stateCounts();
    const timeoutAssignments = timeout.child.__testSrcdocAssignments;
    retireOuter(timeout);
    const pagehide = await makeOuter("pagehide", policy, 60000);
    sendBegin(pagehide, "pagehide-session", "surface-pagehide", artifactDigest,
      "pagehide-begin");
    await waitFor(pagehide.messages, message =>
      message.type === "nmp.outer.mount.begin.result" && message.ok === true,
    "pagehide-begin");
    pagehide.child.dispatchEvent(new pagehide.child.PageTransitionEvent("pagehide"));
    const pagehideCounts = pagehide.child.__chunkTestBridge.stateCounts();
    const pagehideAssignments = pagehide.child.__testSrcdocAssignments;
    window.removeEventListener("message", pagehide.listener);
    pagehide.frame.remove();
    return {
      chunkCountExceedsWindow: chunkCount > 256,
      splitUtf8Preserved: artifactBytes[CHUNK - 1] === 0xE2 &&
        artifactBytes[CHUNK] === 0x82 && artifactBytes[CHUNK + 1] === 0xAC,
      exactMaterializedDigest: positiveDigest === materializedDigest,
      commitResults,
      srcdocAssignments,
      timeoutExpired: expired.error === "transfer-expired",
      timeoutStateRetired: timeoutCounts.pendingTransfers === 0 &&
        timeoutCounts.reservedTransferBytes === 0,
      pagehideStateRetired: pagehideCounts.pendingTransfers === 0 &&
        pagehideCounts.reservedTransferBytes === 0,
      refusedSrcdocAssignments: timeoutAssignments + pagehideAssignments,
      errors
    };
    """#
}

@MainActor
private final class ChunkTransportNavigationProbe: NSObject, WKNavigationDelegate {
    private let loaded: XCTestExpectation
    init(_ loaded: XCTestExpectation) {
        self.loaded = loaded
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loaded.fulfill()
    }
}
