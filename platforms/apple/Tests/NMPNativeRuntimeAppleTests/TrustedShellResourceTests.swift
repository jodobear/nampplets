import Foundation
import Network
import Testing
import WebKit
import XCTest
@testable import NMPNativeRuntimeApple

@Suite("Trusted Web Shell")
struct TrustedShellResourceTests {
    @Test("bundled artifact remains an unchanged compatibility fixture")
    func fixtureLoads() throws {
        let artifact = try #require(NappletArtifact.bundledCompatibilityFixture())

        #expect(artifact.html.contains("window.napplet.shell.ping"))
        #expect(!artifact.html.contains("window.webkit"))
        #expect(!artifact.html.contains("window.nostr ="))
    }

    @Test("shell source binds the exact iframe source and denies ambient network")
    func sourceBindingAndCSP() throws {
        let shellURL = try #require(TrustedShellResources.shellURL)
        let directory = shellURL.deletingLastPathComponent()
        let script = try String(
            contentsOf: directory.appendingPathComponent("trusted-shell.js"),
            encoding: .utf8
        )
        let document = try String(contentsOf: shellURL, encoding: .utf8)

        #expect(script.contains("event.source !== frame.contentWindow"))
        #expect(script.contains("setAttribute(\"sandbox\", \"allow-scripts\")"))
        #expect(!script.contains("allow-same-origin"))
        #expect(script.contains("new global.DOMParser()"))
        #expect(!script.contains("artifactHTML.replace"))
        #expect(document.contains("connect-src 'none'"))
    }
}

@MainActor
final class TrustedNappletRoundTripTests: XCTestCase {
    func testMappedCanaryEnvelopeReachesTheNativeIsolationBoundary() async throws {
        let artifact = try XCTUnwrap(NappletArtifact.bundledCompatibilityFixture())
        let requestAccepted = expectation(
            description: "native host accepts shell.ping from the mapped iframe"
        )
        var activities: [TrustedNappletActivity] = []
        let view = TrustedNappletView(artifact: artifact) { activity in
            activities.append(activity)
            if activity == .request(type: "shell.ping") {
                requestAccepted.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [requestAccepted], timeout: 10)
        if !activities.contains(.request(type: "shell.ping")) {
            XCTFail("Observed activities: \(activities)")
        }
    }

    func testBootstrapPrecedesAScriptAuthoredBeforeHead() async throws {
        let artifact = NappletArtifact(
            title: "Early script",
            html: """
            <!doctype html>
            <script>
              window.napplet.shell.ping({ source: "before-head" });
            </script>
            <html><head><title>Early script</title></head><body></body></html>
            """
        )
        let requestAccepted = expectation(
            description: "parser places bootstrap before an early authored script"
        )
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .request(type: "shell.ping") {
                requestAccepted.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [requestAccepted], timeout: 10)
    }

    func testSandboxBlocksEveryAmbientNetworkPrimitiveBeforeTransport() async throws {
        let listener = try NWListener(using: .tcp, on: .any)
        let listenerReady = expectation(description: "local connection probe is listening")
        let unexpectedConnection = expectation(
            description: "sandboxed napplet reached the network transport"
        )
        unexpectedConnection.isInverted = true
        let connections = LockedCounter()
        listener.stateUpdateHandler = { state in
            if case .ready = state {
                listenerReady.fulfill()
            }
        }
        listener.newConnectionHandler = { connection in
            connections.increment()
            unexpectedConnection.fulfill()
            connection.cancel()
        }
        listener.start(queue: DispatchQueue(label: "io.f7z.nmp.native-runtime.network-probe"))
        defer { listener.cancel() }
        await fulfillment(of: [listenerReady], timeout: 5)
        let port = try XCTUnwrap(listener.port?.rawValue)
        let httpOrigin = "http://127.0.0.1:\(port)"
        let websocketOrigin = "ws://127.0.0.1:\(port)"

        let artifact = NappletArtifact(
            title: "Network denial",
            html: """
            <!doctype html><html><head></head><body><script>
            (function () {
              const pending = new Set([
                "fetch", "websocket", "image", "script", "style",
                "beacon", "eventsource"
              ]);
              function complete(name) {
                pending.delete(name);
                if (pending.size === 0) {
                  window.napplet.shell.ping({ source: "network-denial" });
                }
              }

              fetch("\(httpOrigin)/fetch").then(
                function () { complete("fetch"); },
                function () { complete("fetch"); }
              );

              try {
                const socket = new WebSocket("\(websocketOrigin)/websocket");
                socket.onerror = function () { complete("websocket"); };
                socket.onopen = function () {
                  socket.close();
                  complete("websocket");
                };
              } catch (_) {
                complete("websocket");
              }

              const image = new Image();
              image.onload = image.onerror = function () { complete("image"); };
              image.src = "\(httpOrigin)/image.png";

              const script = document.createElement("script");
              script.onload = script.onerror = function () { complete("script"); };
              script.src = "\(httpOrigin)/script.js";
              document.head.appendChild(script);

              const style = document.createElement("link");
              style.rel = "stylesheet";
              style.onload = style.onerror = function () { complete("style"); };
              style.href = "\(httpOrigin)/style.css";
              document.head.appendChild(style);

              try {
                navigator.sendBeacon("\(httpOrigin)/beacon", "blocked");
              } finally {
                complete("beacon");
              }

              try {
                const source = new EventSource("\(httpOrigin)/events");
                source.onerror = function () {
                  source.close();
                  complete("eventsource");
                };
              } catch (_) {
                complete("eventsource");
              }
            })();
            </script></body></html>
            """
        )
        let denialComplete = expectation(
            description: "all denied primitives reported completion from WebKit"
        )
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .request(type: "shell.ping") {
                denialComplete.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [denialComplete], timeout: 10)
        await fulfillment(of: [unexpectedConnection], timeout: 0.5)
        XCTAssertEqual(connections.value, 0)
    }

    func testNappletCannotReachHostDOMStorageWorkersNativeBridgeOrWindowNostr() async throws {
        let artifact = NappletArtifact(
            title: "Capability isolation",
            html: """
            <!doctype html><html><head></head><body><script>
            (function () {
              let hostDOMDenied = false;
              let localStorageDenied = false;
              let sessionStorageDenied = false;
              let cookieDenied = false;
              try {
                void parent.document.documentElement;
              } catch (_) {
                hostDOMDenied = true;
              }
              try {
                localStorage.setItem("nmp-native-probe", "forbidden");
              } catch (_) {
                localStorageDenied = true;
              }
              try {
                sessionStorage.setItem("nmp-native-probe", "forbidden");
              } catch (_) {
                sessionStorageDenied = true;
              }
              try {
                document.cookie = "nmp-native-probe=forbidden; SameSite=Strict";
                cookieDenied = document.cookie.indexOf("nmp-native-probe=") === -1;
              } catch (_) {
                cookieDenied = true;
              }

              const indexedDBDenied = new Promise(function (resolve) {
                try {
                  const request = indexedDB.open("nmp-native-forbidden");
                  request.onerror = function () { resolve(true); };
                  request.onsuccess = function () {
                    request.result.close();
                    indexedDB.deleteDatabase("nmp-native-forbidden");
                    resolve(false);
                  };
                } catch (_) {
                  resolve(true);
                }
              });

              const serviceWorkerDenied = new Promise(function (resolve) {
                try {
                  if (!("serviceWorker" in navigator)) {
                    resolve(true);
                    return;
                  }
                  navigator.serviceWorker.register("data:text/javascript,")
                    .then(function (registration) {
                      registration.unregister();
                      resolve(false);
                    })
                    .catch(function () { resolve(true); });
                } catch (_) {
                  resolve(true);
                }
              });

              const immediateChecks = [
                hostDOMDenied,
                localStorageDenied,
                sessionStorageDenied,
                cookieDenied,
                !(
                   window.webkit &&
                   window.webkit.messageHandlers &&
                   window.webkit.messageHandlers.runtimeBridge
                ),
                typeof window.nostr === "undefined"
              ];
              if (immediateChecks.every(Boolean)) {
                Promise.all([indexedDBDenied, serviceWorkerDenied]).then(function (asyncChecks) {
                  if (asyncChecks.every(Boolean)) {
                    window.napplet.shell.ping({ source: "capability-isolation" });
                  }
                });
              } else {
                // Keep the promise handlers installed so a failed immediate
                // check cannot surface as an unhandled rejection.
                Promise.all([indexedDBDenied, serviceWorkerDenied]).then(function () {});
              }
            })();
            </script></body></html>
            """
        )
        let isolationComplete = expectation(
            description: "sandbox reports all ambient capabilities absent"
        )
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .request(type: "shell.ping") {
                isolationComplete.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [isolationComplete], timeout: 10)
    }

    func testSiblingFrameCannotSpoofTheMappedNappletSource() async throws {
        let artifact = NappletArtifact(
            title: "Mapped source",
            html: """
            <!doctype html><html><head></head><body><script>
            addEventListener("message", function (event) {
              if (event.source === parent && event.data === "send-legitimate-ping") {
                window.napplet.shell.ping({ source: "mapped-frame" });
              }
            });
            </script></body></html>
            """
        )
        let mounted = expectation(description: "trusted shell mounted")
        let firstRequest = expectation(description: "mapped frame request arrived")
        let secondRequest = expectation(description: "a spoofed second request arrived")
        secondRequest.isInverted = true
        var requestCount = 0
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .mounted {
                mounted.fulfill()
            }
            if activity == .request(type: "shell.ping") {
                requestCount += 1
                if requestCount == 1 {
                    firstRequest.fulfill()
                } else {
                    secondRequest.fulfill()
                }
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [mounted], timeout: 10)
        _ = try await webView.callAsyncJavaScript(
            """
            return await new Promise(function (resolve) {
              const sibling = document.createElement("iframe");
              sibling.setAttribute("sandbox", "allow-scripts");
              sibling.onload = function () {
                document.getElementById("napplet-frame").contentWindow.postMessage(
                  "send-legitimate-ping",
                  "*"
                );
                resolve(true);
              };
              sibling.srcdoc = `<script>
                parent.postMessage({
                  type: "shell.ping",
                  requestId: "spoofed-sibling"
                }, "*");
              <\\/script>`;
              document.getElementById("surface").appendChild(sibling);
            });
            """,
            arguments: [:],
            in: nil,
            contentWorld: .page
        )

        await fulfillment(of: [firstRequest], timeout: 10)
        await fulfillment(of: [secondRequest], timeout: 0.5)
        XCTAssertEqual(requestCount, 1)
    }

    func testCallerSuppliedSessionFieldDoesNotBecomeNativeAuthority() async throws {
        let artifact = NappletArtifact(
            title: "Session spoof",
            html: """
            <!doctype html><html><head></head><body><script>
              parent.postMessage({
                type: "shell.ping",
                requestId: "caller-session-spoof",
                session: "attacker-controlled"
              }, "*");
            </script></body></html>
            """
        )
        let requestAccepted = expectation(
            description: "native accepts the mapped request without trusting its session field"
        )
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .request(type: "shell.ping") {
                requestAccepted.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [requestAccepted], timeout: 10)
    }

    func testOnlyTheExactTrustedShellFileMayNavigateTheMainFrame() async throws {
        let artifact = NappletArtifact(
            title: "Exact navigation",
            html: "<!doctype html><html><head></head><body></body></html>"
        )
        let mounted = expectation(description: "trusted shell mounted")
        let denied = expectation(description: "sibling file navigation denied")
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .mounted {
                mounted.fulfill()
            }
            if activity == .refused(reason: "Trusted shell navigation was denied") {
                denied.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [mounted], timeout: 10)
        let shellURL = try XCTUnwrap(TrustedShellResources.shellURL)
        let siblingURL = shellURL.deletingLastPathComponent()
            .appendingPathComponent("trusted-shell.js")
        webView.loadFileURL(
            siblingURL,
            allowingReadAccessTo: shellURL.deletingLastPathComponent()
        )

        await fulfillment(of: [denied], timeout: 5)
        XCTAssertEqual(
            webView.url?.resolvingSymlinksInPath().standardizedFileURL,
            shellURL.resolvingSymlinksInPath().standardizedFileURL
        )
    }

    func testBridgeReceiptRequiresTheActiveTrustedNavigationGeneration() async throws {
        let artifact = NappletArtifact(
            title: "Navigation generation",
            html: "<!doctype html><html><head></head><body></body></html>"
        )
        let mounted = expectation(description: "trusted shell mounted")
        let refused = expectation(description: "inactive navigation generation refused")
        refused.assertForOverFulfill = false
        let unexpectedRequest = expectation(
            description: "inactive navigation generation reached the router"
        )
        unexpectedRequest.isInverted = true
        let view = TrustedNappletView(artifact: artifact) { activity in
            if activity == .mounted {
                mounted.fulfill()
            }
            if activity == .refused(
                reason: "Bridge message did not originate in the trusted main frame"
            ) {
                refused.fulfill()
            }
            if activity == .request(type: "shell.ping") {
                unexpectedRequest.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        await fulfillment(of: [mounted], timeout: 10)
        coordinator.webView(webView, didStartProvisionalNavigation: nil)
        _ = try await webView.callAsyncJavaScript(
            """
            document.documentElement.setAttribute(
              "data-nmp-native-envelope",
              JSON.stringify({
                session: "stale-navigation",
                envelope: { type: "shell.ping", requestId: "stale-navigation" }
              })
            );
            document.dispatchEvent(new Event("nmp-native-envelope"));
            return true;
            """,
            arguments: [:],
            in: nil,
            contentWorld: .page
        )

        await fulfillment(of: [refused], timeout: 5)
        await fulfillment(of: [unexpectedRequest], timeout: 0.5)
    }

    func testStopRemovesTheNativeBridgeAndIsIdempotent() async throws {
        let artifact = NappletArtifact(
            title: "Teardown",
            html: "<!doctype html><html><head></head><body></body></html>"
        )
        let mounted = expectation(description: "trusted shell mounted")
        let unexpectedRequest = expectation(description: "stopped bridge accepted a request")
        unexpectedRequest.isInverted = true
        let unexpectedActivity = expectation(description: "stopped coordinator emitted activity")
        unexpectedActivity.isInverted = true
        let hasStopped = LockedFlag()
        let view = TrustedNappletView(artifact: artifact) { activity in
            if hasStopped.value {
                unexpectedActivity.fulfill()
            }
            if activity == .mounted {
                mounted.fulfill()
            }
            if activity == .request(type: "shell.ping") {
                unexpectedRequest.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()

        await fulfillment(of: [mounted], timeout: 10)
        coordinator.stop(webView)
        coordinator.stop(webView)
        hasStopped.set()
        XCTAssertNil(webView.navigationDelegate)
        coordinator.webViewWebContentProcessDidTerminate(webView)
        coordinator.webView(webView, didFinish: nil)
        _ = try? await webView.callAsyncJavaScript(
            """
            document.documentElement.setAttribute(
              "data-nmp-native-envelope",
              JSON.stringify({
                session: "after-stop",
                envelope: { type: "shell.ping", requestId: "after-stop" }
              })
            );
            document.dispatchEvent(new Event("nmp-native-envelope"));
            return true;
            """,
            arguments: [:],
            in: nil,
            contentWorld: .page
        )
        await fulfillment(of: [unexpectedRequest], timeout: 0.5)
        await fulfillment(of: [unexpectedActivity], timeout: 0.5)
    }

    func testContentProcessTerminationSurfacesCrashState() {
        let crashed = expectation(description: "crash state surfaced")
        let view = TrustedNappletView(
            artifact: NappletArtifact(
                title: "Crash",
                html: "<!doctype html><html><head></head><body></body></html>"
            )
        ) { activity in
            if activity == .crashed {
                crashed.fulfill()
            }
        }
        let coordinator = view.makeCoordinator()
        let webView = coordinator.makeWebView()
        defer { coordinator.stop(webView) }

        coordinator.webViewWebContentProcessDidTerminate(webView)
        wait(for: [crashed], timeout: 1)
    }
}

private final class LockedCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = 0

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func increment() {
        lock.lock()
        storage += 1
        lock.unlock()
    }
}

private final class LockedFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var storage = false

    var value: Bool {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func set() {
        lock.lock()
        storage = true
        lock.unlock()
    }
}
