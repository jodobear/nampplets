"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const {
  MAX_PARENT_MESSAGES_PER_SECOND,
  createEmbeddingBridge
} = require("../trusted-shell-embedding.js");
const {
  ARTIFACT_CHUNK_BYTES
} = require("../trusted-shell-embedding-transfer.js");
const {
  checkEmbeddedShell,
  renderEmbeddedShell,
  scriptNames
} = require("../scripts/render-trusted-shell-embedded.js");

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function createHarness(options = {}) {
  const parent = {
    posted: [],
    postMessage(message, target) { this.posted.push({ message, target }); }
  };
  const listeners = new Map();
  const calls = {
    mounts: [], receives: [], unmounts: [], disposed: 0,
    materializations: 0, srcdocAssignments: 0
  };
  let currentTime = 1000;
  let generation = 0;
  let disposed = false;
  const active = new Set();
  let forwardEnvelope;
  const primitives = {
    isPlainObject(value) {
      return value !== null && typeof value === "object" && !Array.isArray(value);
    },
    isVerifiedArtifactBaseURL(value) {
      return value === "nmp-artifact://00000000-0000-4000-8000-000000000001/";
    },
    materialize(html, base, domains) {
      calls.materializations += 1;
      return `materialized:${html}:${base}:${domains.join(",")}`;
    }
  };
  const hostModule = {
    MAX_ARTIFACT_HTML_BYTES: 8 * 1024 * 1024,
    createSurfaceHost(_environment, _primitives, options) {
      forwardEnvelope = options.forwardEnvelope;
      return {
        async verifyAndMaterialize(
          binding, artifactHTML, materialize, digestText, isCurrent
        ) {
          const currentGeneration = generation;
          const current = () => !disposed && generation === currentGeneration &&
            isCurrent();
          const artifactDigest = await digestText(artifactHTML);
          if (artifactDigest !== binding.artifactDigest) {
            return Object.freeze({ status: "digest-mismatch" });
          }
          if (!current()) return Object.freeze({ status: "stale" });
          const materializedHTML = materialize();
          const materializedDigest = await digestText(materializedHTML);
          if (!current()) return Object.freeze({ status: "stale" });
          return Object.freeze({
            status: "verified", materializedHTML, materializedDigest
          });
        },
        invalidateArtifactVerification() { generation += 1; },
        mount(surfaceId, surface, configuration) {
          calls.srcdocAssignments += 1;
          calls.mounts.push({ surfaceId, surface, configuration });
          active.add(surfaceId);
          return true;
        },
        receive(surfaceId, envelope) {
          calls.receives.push({ surfaceId, envelope });
          return active.has(surfaceId) && envelope && typeof envelope.type === "string";
        },
        unmount(surfaceId) {
          calls.unmounts.push(surfaceId);
          return active.delete(surfaceId);
        },
        dispose() {
          calls.disposed += 1;
          disposed = true;
          generation += 1;
          active.clear();
        }
      };
    }
  };
  const surface = {};
  const environment = {
    parent,
    document: { getElementById(id) { return id === "surface" ? surface : null; } },
    addEventListener(type, listener) { listeners.set(type, listener); },
    removeEventListener(type) { listeners.delete(type); }
  };
  const bridge = createEmbeddingBridge(environment, {
    primitives,
    hostModule,
    digestText: options.digestText || (async (value) => digest(value)),
    now: () => currentTime
  });
  return {
    bridge,
    advance(milliseconds) { currentTime += milliseconds; },
    calls,
    environment,
    forward(message) { forwardEnvelope(message); },
    listeners,
    parent,
    primitives,
    surface
  };
}

function binding(surface, session, artifactHTML) {
  return {
    manifestAuthor: "a".repeat(64),
    dTag: "clock",
    aggregateHash: "b".repeat(64),
    artifactDigest: digest(artifactHTML),
    surface,
    session
  };
}

function mountRequest(
  harness,
  session,
  artifactHTML = "<p>verified</p>",
  surfaceId = "surface-a"
) {
  const artifactBaseURL =
    "nmp-artifact://00000000-0000-4000-8000-000000000001/";
  const domains = ["shell"];
  return {
    type: "nmp.outer.mount",
    requestId: `mount-${session}`,
    surfaceId,
    configuration: {
      session,
      artifactHTML,
      artifactBaseURL,
      domains,
      title: "Clock",
      binding: binding(surfaceId, session, artifactHTML)
    }
  };
}

async function dispatch(harness, data, source = harness.parent) {
  harness.listeners.get("message")({ source, data });
  await new Promise((resolve) => setImmediate(resolve));
}

function chunkedMountRequests(
  harness,
  session,
  artifactHTML,
  surfaceId = "surface-a"
) {
  const bytes = new TextEncoder().encode(artifactHTML);
  const full = mountRequest(harness, session, artifactHTML, surfaceId);
  const configuration = { ...full.configuration };
  delete configuration.artifactHTML;
  configuration.artifactBytes = bytes.byteLength;
  const transferId = `begin-${session}`;
  const requests = [{
    type: "nmp.outer.mount.begin",
    requestId: transferId,
    surfaceId,
    configuration
  }];
  for (let offset = 0; offset < bytes.byteLength; offset += ARTIFACT_CHUNK_BYTES) {
    requests.push({
      type: "nmp.outer.mount.chunk",
      requestId: `chunk-${session}-${offset}`,
      surfaceId,
      session,
      transferId,
      offset,
      bytes: bytes.slice(offset, offset + ARTIFACT_CHUNK_BYTES).buffer
    });
  }
  requests.push({
    type: "nmp.outer.mount.commit",
    requestId: `commit-${session}`,
    surfaceId,
    session,
    transferId
  });
  return requests;
}

test("embedded host binds parent, artifact digests, and napplet forwarding", async () => {
  const harness = createHarness();
  assert.deepEqual(harness.parent.posted[0], {
    message: { type: "nmp.outer.ready", version: 1 },
    target: "*"
  });
  const request = mountRequest(harness, "session-a");
  await dispatch(harness, request, {});
  assert.equal(harness.calls.mounts.length, 0);
  await dispatch(harness, request);
  assert.equal(harness.calls.mounts.length, 1);
  assert.equal(harness.calls.materializations, 1);
  assert.equal(harness.calls.mounts[0].surface, harness.surface);
  assert.equal(
    harness.calls.mounts[0].configuration.materializedHTML,
    `materialized:${request.configuration.artifactHTML}:` +
      `${request.configuration.artifactBaseURL}:shell`
  );
  assert.equal(harness.parent.posted.at(-1).message.ok, true);
  assert.equal(
    harness.parent.posted.at(-1).message.binding.materializedDigest,
    digest(`materialized:${request.configuration.artifactHTML}:` +
      `${request.configuration.artifactBaseURL}:shell`)
  );

  harness.forward({
    surfaceId: "surface-a",
    session: "session-a",
    envelope: { type: "shell.ready" }
  });
  assert.equal(harness.parent.posted.at(-1).message.type, "nmp.outer.napplet");
  assert.equal(harness.parent.posted.at(-1).message.binding.dTag, "clock");

  const mounted = harness.calls.mounts[0].configuration;
  mounted.onReady();
  assert.equal(harness.parent.posted.at(-1).message.type, "nmp.outer.surface.ready");
});

test("ordered 256 KiB mount chunks reconstruct before the sealed sink", async () => {
  const harness = createHarness();
  const artifactHTML = "x".repeat(ARTIFACT_CHUNK_BYTES + 1);
  const requests = chunkedMountRequests(
    harness, "chunked-session", artifactHTML
  );
  for (const request of requests) await dispatch(harness, request);
  assert.equal(harness.calls.mounts.length, 1);
  assert.equal(
    harness.calls.mounts[0].configuration.artifactHTML,
    artifactHTML
  );
  assert.equal(harness.parent.posted.at(-1).message.ok, true);
  assert.deepEqual(harness.bridge.stateCounts(), {
    bindings: 1,
    pendingMounts: 0,
    pendingSurfaces: 0,
    pendingTransfers: 0,
    reservedTransferBytes: 0,
    elevatedPending: 0,
    elevatedActive: 0,
    reservedArtifactHTMLBytes: 0,
    reservedMaterializedHTMLBytes: 0
  });
});

test("chunk gaps and replay retire bytes without assigning srcdoc", async () => {
  const harness = createHarness();
  const artifactHTML = "x".repeat(ARTIFACT_CHUNK_BYTES + 1);
  const requests = chunkedMountRequests(harness, "replayed", artifactHTML);
  await dispatch(harness, requests[0]);
  await dispatch(harness, requests[1]);
  await dispatch(harness, requests[1]);
  assert.equal(harness.parent.posted.at(-1).message.error, "transfer-refused");
  await dispatch(harness, requests.at(-1));
  assert.equal(harness.parent.posted.at(-1).message.error, "stale");
  assert.equal(harness.calls.materializations, 0);
  assert.equal(harness.calls.srcdocAssignments, 0);
  assert.equal(harness.bridge.stateCounts().pendingTransfers, 0);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);
});

test("oversized chunks, remount, unmount, and teardown retire transfer bytes", async () => {
  const artifactHTML = "x".repeat(ARTIFACT_CHUNK_BYTES + 1);
  const oversizedHarness = createHarness();
  const oversized = chunkedMountRequests(
    oversizedHarness, "oversized", artifactHTML
  );
  await dispatch(oversizedHarness, oversized[0]);
  oversized[1].bytes = new ArrayBuffer(ARTIFACT_CHUNK_BYTES + 1);
  await dispatch(oversizedHarness, oversized[1]);
  assert.equal(oversizedHarness.parent.posted.at(-1).message.error,
    "transfer-refused");
  assert.equal(oversizedHarness.bridge.stateCounts().reservedTransferBytes, 0);

  const harness = createHarness();
  const old = chunkedMountRequests(harness, "old-transfer", artifactHTML);
  const current = chunkedMountRequests(harness, "new-transfer", artifactHTML);
  await dispatch(harness, old[0]);
  await dispatch(harness, current[0]);
  await dispatch(harness, old.at(-1));
  assert.equal(harness.parent.posted.at(-1).message.error, "stale");
  const malformed = { ...current[1], bytes: "not-an-array-buffer" };
  await dispatch(harness, malformed);
  assert.equal(harness.parent.posted.at(-1).message.error, "transfer-refused");
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);
  await dispatch(harness, current[0]);
  await dispatch(harness, {
    type: "nmp.outer.unmount",
    requestId: "cancel-current",
    surfaceId: "surface-a",
    session: "new-transfer"
  });
  assert.equal(harness.parent.posted.at(-1).message.ok, true);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);

  const final = chunkedMountRequests(harness, "pagehide-transfer", artifactHTML);
  await dispatch(harness, final[0]);
  harness.listeners.get("pagehide")();
  assert.equal(harness.bridge.stateCounts().pendingTransfers, 0);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);
  assert.equal(harness.listeners.has("message"), false);
});

test("one-byte mutation and type-confused launch refuse before the sealed sink", async () => {
  const harness = createHarness();
  const request = mountRequest(harness, "session-a");
  request.configuration.artifactHTML += "x";
  await dispatch(harness, request);
  assert.equal(harness.calls.materializations, 0);
  assert.equal(harness.calls.srcdocAssignments, 0);
  assert.equal(harness.calls.mounts.length, 0);
  assert.equal(harness.parent.posted.at(-1).message.error, "digest-mismatch");

  const confused = mountRequest(harness, "session-b");
  confused.configuration.untrusted = true;
  const count = harness.parent.posted.length;
  await dispatch(harness, confused);
  assert.equal(harness.calls.mounts.length, 0);
  assert.equal(harness.parent.posted.length, count);
});

test("parent rate overflow reports once, resets, and teardown is terminal", async () => {
  const harness = createHarness();
  const receive = harness.listeners.get("message");
  for (let index = 0; index < MAX_PARENT_MESSAGES_PER_SECOND + 8; index += 1) {
    receive({ source: harness.parent, data: { type: "unknown" } });
  }
  assert.deepEqual(
    harness.parent.posted.filter(({ message }) =>
      message.type === "nmp.outer.rate-limited"
    ).map(({ message }) => message),
    [{ type: "nmp.outer.rate-limited", scope: "parent" }]
  );

  harness.advance(1000);
  for (let index = 0; index < MAX_PARENT_MESSAGES_PER_SECOND + 1; index += 1) {
    receive({ source: harness.parent, data: { type: "unknown" } });
  }
  assert.equal(harness.parent.posted.filter(({ message }) =>
    message.type === "nmp.outer.rate-limited").length, 2);

  harness.listeners.get("pagehide")();
  assert.equal(harness.listeners.has("message"), false);
});

test("remount drops stale session traffic and dispose closes every listener", async () => {
  const harness = createHarness();
  await dispatch(harness, mountRequest(harness, "session-old"));
  await dispatch(harness, mountRequest(harness, "session-new"));
  const before = harness.parent.posted.length;
  harness.forward({
    surfaceId: "surface-a",
    session: "session-old",
    envelope: { type: "shell.ready" }
  });
  assert.equal(harness.parent.posted.length, before);

  await dispatch(harness, {
    type: "nmp.outer.deliver",
    requestId: "stale",
    surfaceId: "surface-a",
    session: "session-old",
    envelope: { type: "identity.changed" }
  });
  assert.equal(harness.parent.posted.at(-1).message.error, "stale");
  await dispatch(harness, {
    type: "nmp.outer.unmount",
    requestId: "unmount",
    surfaceId: "surface-a",
    session: "session-new"
  });
  assert.equal(harness.parent.posted.at(-1).message.ok, true);

  harness.listeners.get("pagehide")();
  assert.equal(harness.calls.disposed, 1);
  assert.equal(harness.listeners.has("message"), false);
  assert.equal(harness.listeners.has("pagehide"), false);
});

test("stale asynchronous mounts retire without replacing the current surface", async () => {
  let releaseOldDigest;
  const harness = createHarness({
    digestText(value) {
      if (value === "<p>old</p>") {
        return new Promise((resolve) => {
          releaseOldDigest = () => resolve(digest(value));
        });
      }
      return Promise.resolve(digest(value));
    }
  });
  const receive = harness.listeners.get("message");
  receive({
    source: harness.parent,
    data: mountRequest(harness, "session-old", "<p>old</p>")
  });
  await dispatch(harness, mountRequest(harness, "session-new", "<p>new</p>"));
  releaseOldDigest();
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(harness.calls.mounts.map(({ configuration }) =>
    configuration.session), ["session-new"]);
  assert.deepEqual(harness.bridge.stateCounts(), {
    bindings: 1,
    pendingMounts: 0,
    pendingSurfaces: 0,
    pendingTransfers: 0,
    reservedTransferBytes: 0,
    elevatedPending: 0,
    elevatedActive: 0,
    reservedArtifactHTMLBytes: 0,
    reservedMaterializedHTMLBytes: 0
  });
});

test("unique refused and unmounted surfaces return state to baseline", async () => {
  const harness = createHarness();
  for (let index = 0; index < 40; index += 1) {
    const request = mountRequest(
      harness,
      `refused-${index}`,
      `<p>refused-${index}</p>`,
      `refused-${index}`
    );
    request.configuration.binding.artifactDigest = "0".repeat(64);
    await dispatch(harness, request);
  }
  for (let index = 0; index < 40; index += 1) {
    const surfaceId = `mounted-${index}`;
    const session = `session-${index}`;
    await dispatch(harness, mountRequest(
      harness,
      session,
      `<p>mounted-${index}</p>`,
      surfaceId
    ));
    await dispatch(harness, {
      type: "nmp.outer.unmount",
      requestId: `unmount-${index}`,
      surfaceId,
      session
    });
  }
  assert.deepEqual(harness.bridge.stateCounts(), {
    bindings: 0,
    pendingMounts: 0,
    pendingSurfaces: 0,
    pendingTransfers: 0,
    reservedTransferBytes: 0,
    elevatedPending: 0,
    elevatedActive: 0,
    reservedArtifactHTMLBytes: 0,
    reservedMaterializedHTMLBytes: 0
  });
});

test("deliver and unmount identifiers are bounded and never reflected", async () => {
  const harness = createHarness();
  await dispatch(harness, mountRequest(harness, "bounded-session"));
  const invalidIdentifiers = [
    { surfaceId: {}, session: "bounded-session" },
    { surfaceId: "surface-a", session: [] },
    { surfaceId: "bad\u0000surface", session: "bounded-session" },
    { surfaceId: "surface-a", session: "bad\u0000session" },
    { surfaceId: "s".repeat(129), session: "bounded-session" },
    { surfaceId: "surface-a", session: "s".repeat(257) }
  ];
  for (const [index, identifiers] of invalidIdentifiers.entries()) {
    for (const request of [{
      type: "nmp.outer.deliver",
      requestId: `invalid-deliver-${index}`,
      ...identifiers,
      envelope: { type: "identity.changed" }
    }, {
      type: "nmp.outer.unmount",
      requestId: `invalid-unmount-${index}`,
      ...identifiers
    }]) {
      const posted = harness.parent.posted.length;
      await dispatch(harness, request);
      assert.equal(harness.parent.posted.length, posted);
    }
  }
  assert.equal(harness.calls.receives.length, 0);
  assert.equal(harness.bridge.stateCounts().bindings, 1);
});

test("bound delivery refusal returns a fixed error", async () => {
  const harness = createHarness();
  await dispatch(harness, mountRequest(harness, "session-a"));
  await dispatch(harness, {
    type: "nmp.outer.deliver",
    requestId: "invalid-envelope",
    surfaceId: "surface-a",
    session: "session-a",
    envelope: { invalid: true }
  });
  assert.deepEqual(harness.parent.posted.at(-1).message, {
    type: "nmp.outer.deliver.result",
    requestId: "invalid-envelope",
    surfaceId: "surface-a",
    session: "session-a",
    ok: false,
    error: "deliver-refused",
    binding: null
  });
});

test("generated outer shell has one sealed HTML sink and pinned immutable bytes", () => {
  const root = path.join(__dirname, "..");
  const host = fs.readFileSync(path.join(root, "trusted-shell-surface-host.js"), "utf8");
  const embedding = fs.readFileSync(path.join(root, "trusted-shell-embedding.js"), "utf8");
  const embedded = fs.readFileSync(path.join(root, "trusted-shell-embedded.html"));
  const recorded = fs.readFileSync(
    path.join(root, "trusted-shell-embedded.sha256"),
    "utf8"
  ).trim().split(/\s+/)[0];
  const sources = `${host}\n${embedding}`;
  assert.deepEqual(scriptNames, [
    "trusted-shell-policy.js",
    "trusted-shell-prelude-domains.js",
    "trusted-shell.js",
    "trusted-shell-artifact-policy.js",
    "trusted-shell-artifact-verifier.js",
    "trusted-shell-surface-host.js",
    "trusted-shell-embedding-contract.js",
    "trusted-shell-embedding-transfer.js",
    "trusted-shell-embedding.js"
  ]);
  assert.equal((sources.match(/\.srcdoc\s*=/g) || []).length, 1);
  assert.doesNotMatch(
    sources,
    /(?:innerHTML|outerHTML)\s*=|insertAdjacentHTML\s*\(|document\.write\s*\(|\beval\s*\(|new Function\s*\(|set(?:Timeout|Interval)\s*\(\s*["']/
  );
  assert.doesNotMatch(embedding, /__TAURI|window\.nostr|fetch\s*\(|XMLHttpRequest|WebSocket|sendBeacon|Worker\s*\(/);
  assert.doesNotMatch(embedding, /createEmbeddingBridge\(global/);
  assert.equal(
    (embedded.toString("utf8").match(/createEmbeddingBridge\(window/g) || []).length,
    1
  );
  assert.equal(crypto.createHash("sha256").update(embedded).digest("hex"), recorded);
  assert.equal(embedded.toString("utf8"), renderEmbeddedShell());
  assert.doesNotThrow(() => checkEmbeddedShell());
  assert.match(embedded.toString("utf8"), /sandbox", "allow-scripts"/);
  assert.doesNotMatch(embedded.toString("utf8"), /allow-same-origin/);
});
