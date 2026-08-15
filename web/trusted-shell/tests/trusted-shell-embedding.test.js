"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

const { createEmbeddingBridge } = require("../trusted-shell-embedding.js");

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function createHarness() {
  const parent = {
    posted: [],
    postMessage(message, target) { this.posted.push({ message, target }); }
  };
  const listeners = new Map();
  const calls = {
    mounts: [], receives: [], unmounts: [], disposed: 0, materializations: 0
  };
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
        mount(surfaceId, surface, configuration) {
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
        dispose() { calls.disposed += 1; active.clear(); }
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
    digestText: async (value) => digest(value)
  });
  return {
    bridge,
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

function mountRequest(harness, session, artifactHTML = "<p>verified</p>") {
  const surfaceId = "surface-a";
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
    harness.primitives.materialize(
      request.configuration.artifactHTML,
      request.configuration.artifactBaseURL,
      request.configuration.domains
    )
  );
  assert.equal(harness.parent.posted.at(-1).message.ok, true);
  assert.equal(
    harness.parent.posted.at(-1).message.binding.materializedDigest,
    digest(harness.primitives.materialize(
      request.configuration.artifactHTML,
      request.configuration.artifactBaseURL,
      request.configuration.domains
    ))
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

test("one-byte mutation and type-confused launch refuse before the sealed sink", async () => {
  const harness = createHarness();
  const request = mountRequest(harness, "session-a");
  request.configuration.artifactHTML += "x";
  await dispatch(harness, request);
  assert.equal(harness.calls.mounts.length, 0);
  assert.equal(harness.parent.posted.at(-1).message.error, "digest-mismatch");

  const confused = mountRequest(harness, "session-b");
  confused.configuration.untrusted = true;
  const count = harness.parent.posted.length;
  await dispatch(harness, confused);
  assert.equal(harness.calls.mounts.length, 0);
  assert.equal(harness.parent.posted.length, count);
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
  assert.equal((sources.match(/\.srcdoc\s*=/g) || []).length, 1);
  assert.doesNotMatch(
    sources,
    /(?:innerHTML|outerHTML)\s*=|insertAdjacentHTML\s*\(|document\.write\s*\(|\beval\s*\(|new Function\s*\(|set(?:Timeout|Interval)\s*\(\s*["']/
  );
  assert.doesNotMatch(embedding, /__TAURI|window\.nostr|fetch\s*\(|XMLHttpRequest|WebSocket|sendBeacon|Worker\s*\(/);
  assert.equal(crypto.createHash("sha256").update(embedded).digest("hex"), recorded);
  assert.match(embedded.toString("utf8"), /sandbox", "allow-scripts"/);
  assert.doesNotMatch(embedded.toString("utf8"), /allow-same-origin/);
});
