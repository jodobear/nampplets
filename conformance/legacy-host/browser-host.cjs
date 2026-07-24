"use strict";

const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { pathToFileURL } = require("node:url");
const { chromium } = require("playwright");

const MAX_REQUEST_BYTES = 8 * 1024 * 1024;
const MAX_CAPTURED_ENVELOPES = 256;
const MAX_CAPTURED_ERRORS = 32;

function readInput() {
  const input = fs.readFileSync(0);
  if (input.length > MAX_REQUEST_BYTES) {
    throw new Error(`runner input exceeds ${MAX_REQUEST_BYTES} bytes`);
  }
  return JSON.parse(input.toString("utf8"));
}

function isInside(root, candidate) {
  const relative = path.relative(root, candidate);
  return relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}

function contentType(file) {
  switch (path.extname(file).toLowerCase()) {
    case ".html":
      return "text/html; charset=utf-8";
    case ".js":
    case ".mjs":
      return "text/javascript; charset=utf-8";
    case ".css":
      return "text/css; charset=utf-8";
    case ".json":
      return "application/json; charset=utf-8";
    case ".svg":
      return "image/svg+xml";
    case ".png":
      return "image/png";
    case ".jpg":
    case ".jpeg":
      return "image/jpeg";
    case ".woff2":
      return "font/woff2";
    default:
      return "application/octet-stream";
  }
}

function createServer(shellRoot, fixtureRoot) {
  const routes = new Map([
    ["/trusted-shell.html", path.join(shellRoot, "trusted-shell.html")],
    ["/trusted-shell.js", path.join(shellRoot, "trusted-shell.js")],
    ["/trusted-shell-policy.js", path.join(shellRoot, "trusted-shell-policy.js")],
    ["/trusted-shell.css", path.join(shellRoot, "trusted-shell.css")],
  ]);

  return http.createServer((request, response) => {
    let file = routes.get(request.url);
    if (!file && request.url.startsWith("/fixture/")) {
      const relative = decodeURIComponent(request.url.slice("/fixture/".length));
      const candidate = path.resolve(fixtureRoot, relative);
      if (isInside(fixtureRoot, candidate)) {
        file = candidate;
      }
    }
    if (!file || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
      response.end("not found");
      return;
    }
    const bytes = fs.readFileSync(file);
    if (bytes.length > MAX_REQUEST_BYTES) {
      response.writeHead(413, { "Content-Type": "text/plain; charset=utf-8" });
      response.end("fixture too large");
      return;
    }
    response.writeHead(200, {
      "Cache-Control": "no-store",
      "Content-Length": String(bytes.length),
      "Content-Type": contentType(file),
      "X-Content-Type-Options": "nosniff",
    });
    response.end(bytes);
  });
}

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("host server did not bind an ephemeral TCP port");
  }
  return address.port;
}

async function runFixture(browser, origin, request, engine) {
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (error) => {
    if (pageErrors.length < MAX_CAPTURED_ERRORS) {
      pageErrors.push(String(error && error.message ? error.message : error));
    }
  });
  const verifiedPaths = new Set(request.verifiedArtifactPaths);
  await page.route("nmp-artifact://**/*", async (route) => {
    let parsed;
    try {
      parsed = new URL(route.request().url());
    } catch (_) {
      await route.abort("blockedbyclient");
      return;
    }
    if (
      parsed.hostname !== "00000000-0000-4000-8000-000000000001" ||
      !verifiedPaths.has(parsed.pathname)
    ) {
      await route.abort("blockedbyclient");
      return;
    }
    const candidate = path.resolve(
      request.fixtureRoot,
      decodeURIComponent(parsed.pathname.slice(1)),
    );
    if (!isInside(request.fixtureRoot, candidate) || !fs.existsSync(candidate)) {
      await route.abort("blockedbyclient");
      return;
    }
    const body = fs.readFileSync(candidate);
    if (body.length > MAX_REQUEST_BYTES) {
      await route.abort("blockedbyclient");
      return;
    }
    await route.fulfill({
      body,
      contentType: contentType(candidate),
      headers: {
        "Cache-Control": "private, immutable",
        "X-Content-Type-Options": "nosniff",
      },
    });
  });

  await page.goto(`${origin}/trusted-shell.html`, {
    waitUntil: "load",
    timeout: request.limits.browserTimeoutMs,
  });
  await page.evaluate((maximum) => {
    globalThis.__legacyHostCaptured = [];
    globalThis.__legacyHostShellInitCount = 0;
    document.addEventListener("nmp-native-envelope", () => {
      if (globalThis.__legacyHostCaptured.length >= maximum) return;
      const raw = document.documentElement.getAttribute("data-nmp-native-envelope");
      if (raw === null) return;
      globalThis.__legacyHostCaptured.push(raw);
      try {
        const message = JSON.parse(raw);
        if (
          message.envelope &&
          message.envelope.type === "shell.ready" &&
          globalThis.__legacyHostShellInitCount === 0
        ) {
          globalThis.__legacyHostShellInitCount = 1;
          queueMicrotask(() => {
            globalThis.__nmpTrustedShellReceive({
              type: "shell.init",
              capabilities: { domains: ["shell"] },
              services: []
            });
          });
        }
      } catch (_) {
        // Malformed bridge frames are reported by the normal decode path.
      }
    });
  }, MAX_CAPTURED_ENVELOPES);

  if (request.preflightReject) {
    const state = await page.evaluate(() => ({
      frameCount: document.querySelectorAll("#napplet-frame").length,
      captured: globalThis.__legacyHostCaptured,
    }));
    await page.close();
    return {
      status: state.frameCount === 0 && state.captured.length === 0 ? "pass" : "fail",
      pre_execution_refusal: true,
      execution_observed: state.frameCount !== 0 || state.captured.length !== 0,
      assertions: {
        fixture_not_mounted: state.frameCount === 0,
        no_envelope_emitted: state.captured.length === 0,
      },
      emitted: [],
      page_errors: pageErrors,
      conformance: {
        status: "not-run",
        reason: "manifest-requires-preflight-refused",
      },
    };
  }

  const artifactHTML = fs.readFileSync(path.join(request.fixtureRoot, "index.html"), "utf8");
  const mounted = await page.evaluate(
    ({ artifactHTML: html, title, artifactBaseURL }) =>
      globalThis.__nmpTrustedShellMount({
        session: "legacy-host-conformance-session",
        artifactHTML: html,
        artifactBaseURL,
        title,
      }),
    {
      artifactHTML,
      title: request.name,
      artifactBaseURL: "nmp-artifact://00000000-0000-4000-8000-000000000001/",
    },
  );

  let frame = null;
  try {
    const frameElement = await page.waitForSelector("#napplet-frame", {
      state: "attached",
      timeout: request.limits.browserTimeoutMs,
    });
    frame = await frameElement.contentFrame();
    if (frame) {
      await frame.waitForLoadState("load", {
        timeout: request.limits.browserTimeoutMs,
      });
    }
  } catch (error) {
    pageErrors.push(`iframe-load: ${String(error && error.message ? error.message : error)}`);
  }

  let nappletState = {
    installedGlobal: false,
    nostrAbsent: false,
    domains: [],
    externalAssetStatus: null,
    publishedFallbackVisible: false,
  };
  if (frame) {
    try {
      await frame.waitForFunction(
        () => window.napplet.shell.supports("shell"),
        undefined,
        { timeout: request.limits.browserTimeoutMs },
      );
      nappletState = await frame.evaluate(() => ({
        installedGlobal: typeof window.napplet === "object" && window.napplet !== null,
        nostrAbsent: typeof window.nostr === "undefined",
        domains:
          typeof window.napplet === "object" && window.napplet !== null
            ? Object.keys(window.napplet).sort()
            : [],
        externalAssetStatus: document.querySelector("#status")?.textContent ?? null,
        publishedFallbackVisible: Boolean(
          document.querySelector("[data-gm-nap-screen], [data-gm-nap-banner]"),
        ),
        shellSupportsShell: window.napplet.shell.supports("shell"),
        shellSupportsUnknown: window.napplet.shell.supports("future-unknown"),
        shellServices: Array.from(window.napplet.shell.services),
      }));
    } catch (error) {
      pageErrors.push(`iframe-observation: ${String(error && error.message ? error.message : error)}`);
    }
  }

  const rawCaptured = await page.evaluate(() => globalThis.__legacyHostCaptured);
  const emitted = [];
  for (const raw of rawCaptured) {
    try {
      const decoded = JSON.parse(raw);
      emitted.push(decoded.envelope);
    } catch (error) {
      pageErrors.push(`bridge-decode: ${String(error && error.message ? error.message : error)}`);
    }
  }

  // @napplet/conformance validates package NAP traffic. The registry-only
  // NAP-SHELL handshake and deliberate host-boundary probes are separate
  // authorities and must not be relabelled as package-domain envelopes.
  const packageEmitted = emitted.filter((envelope) => {
    if (!envelope || typeof envelope.type !== "string") return false;
    const separator = envelope.type.indexOf(".");
    if (separator <= 0) return false;
    return request.packageActiveDomains.includes(envelope.type.slice(0, separator));
  });
  const hostControlOrProbeEmitted = emitted.filter(
    (envelope) => !packageEmitted.includes(envelope),
  );
  const packageRecords = packageEmitted.map((envelope, index) => ({
    envelope,
    verdict: engine.validateEnvelope(envelope),
    timestamp: index,
  }));
  const conformanceContext = engine.buildContext({
    manifestHtml: artifactHTML,
    manifestEvent: request.manifestEvent,
    boot: {
      installedGlobal: nappletState.installedGlobal,
      bootError: pageErrors.length ? pageErrors[0] : null,
      emitted: packageRecords,
      degraded: {
        bootError: pageErrors.length ? pageErrors[0] : null,
        emitted: packageRecords,
      },
    },
    forbiddenGlobals: nappletState.nostrAbsent ? [] : ["window.nostr"],
    sandbox: { allowScripts: true, allowSameOrigin: false },
    lifecycle: null,
  });
  let tick = 0;
  const packageRun = engine.runConformance(conformanceContext, {
    now: () => tick++,
  });

  const packageDomainsAbsent = request.packageActiveDomains.every(
    (domain) => !nappletState.domains.includes(domain),
  );
  const unknownObserved = emitted.some(
    (envelope) => envelope && envelope.type === "future-domain.operation",
  );
  const laterKnownObserved = emitted.some(
    (envelope) => envelope && envelope.type === "theme.get",
  );
  const registryHandshakeObserved = emitted.some(
    (envelope) => envelope && envelope.type === "shell.ready",
  );
  const shellInitCount = await page.evaluate(
    () => globalThis.__legacyHostShellInitCount,
  );

  const assertions = {
    mounted: mounted === true && frame !== null,
    sandbox_allow_scripts_only:
      (await page.evaluate(
        () => document.querySelector("#napplet-frame")?.getAttribute("sandbox") ?? null,
      )) === "allow-scripts",
    prelude_before_authored_scripts: nappletState.installedGlobal,
    window_nostr_absent: nappletState.nostrAbsent,
    package_domains_not_advertised: packageDomainsAbsent,
    registry_shell_handshake_observed: registryHandshakeObserved,
    registry_shell_init_exactly_once: shellInitCount === 1,
    registry_shell_supports_authoritative:
      nappletState.shellSupportsShell === true &&
      nappletState.shellSupportsUnknown === false &&
      Array.isArray(nappletState.shellServices) &&
      nappletState.shellServices.length === 0,
  };
  if (request.name === "external-assets") {
    assertions.external_asset_module_executed =
      nappletState.externalAssetStatus === "ready" &&
      emitted.some(
        (envelope) => envelope && envelope.type === "conformance.external-assets-ready",
      );
  }
  if (request.name === "prelude-order") {
    assertions.first_authored_script_observed_prelude = emitted.some(
      (envelope) =>
        envelope &&
        envelope.type === "conformance.prelude-observed" &&
        envelope.nappletPresent === true &&
        envelope.nostrAbsent === true,
    );
  }
  if (request.name === "unknown-message") {
    assertions.unknown_message_did_not_abort_session =
      unknownObserved && laterKnownObserved && pageErrors.length === 0;
  }
  if (request.classification === "published-immutable-artifacts") {
    assertions.graceful_capability_absence_visible = nappletState.publishedFallbackVisible;
  }

  const requiredAssertions = Object.entries(assertions).filter(
    ([name]) => !name.startsWith("registry_shell_"),
  );
  let status = requiredAssertions.every(([, value]) => value === true) ? "pass" : "fail";
  let reason = null;
  if (
    request.name === "external-assets" &&
    assertions.external_asset_module_executed !== true &&
    pageErrors.length === 0
  ) {
    status = "not-run";
    reason =
      "chromium-harness-cannot-register-the-native-nmp-artifact-url-scheme";
  }
  await page.close();
  return {
    status,
    reason,
    pre_execution_refusal: false,
    execution_observed: frame !== null,
    assertions,
    observed_domains: nappletState.domains,
    emitted,
    package_emitted: packageEmitted,
    host_control_or_probe_emitted: hostControlOrProbeEmitted,
    page_errors: pageErrors,
    conformance: {
      status: packageRun.ok ? "pass" : "fail",
      package: "@napplet/conformance",
      version: request.conformanceVersion,
      run: packageRun,
    },
  };
}

async function main() {
  const input = readInput();
  const engine = await import(pathToFileURL(input.conformanceEntry).href);
  const server = createServer(input.shellRoot, input.fixtureRoot);
  const port = await listen(server);
  const browser = await chromium.launch({
    channel: input.browserChannel,
    headless: true,
  });
  try {
    const result = await runFixture(
      browser,
      `http://127.0.0.1:${port}`,
      input,
      engine,
    );
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    await browser.close();
    if (typeof server.closeAllConnections === "function") {
      server.closeAllConnections();
    }
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  process.stderr.write(`${error && error.stack ? error.stack : String(error)}\n`);
  process.exitCode = 1;
});
