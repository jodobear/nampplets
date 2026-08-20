"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const { createEmbeddingBridge } = require("../trusted-shell-embedding.js");
const policyModule = require("../trusted-shell-artifact-policy.js");

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fixture() {
  const artifactHTML = "x".repeat(
    policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES + 1
  );
  const base = "nmp-artifact://00000000-0000-4000-8000-000000000001/";
  const materializedHTML = `materialized:${artifactHTML}:${base}:shell`;
  return {
    artifactHTML,
    materializedHTML,
    policy: {
      maximumArtifactHTMLBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
      exactArtifactHTMLBytes: Buffer.byteLength(artifactHTML),
      maximumMaterializedHTMLBytes:
        policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES,
      exactMaterializedHTMLBytes: Buffer.byteLength(materializedHTML),
      artifactDigest: digest(artifactHTML),
      materializedDigest: digest(materializedHTML),
      manifestAuthor: "a".repeat(64),
      dTag: "clock",
      aggregateHash: "b".repeat(64),
      exclusive: true
    }
  };
}

function createHarness(artifactPolicy, digestText = async (value) => digest(value)) {
  const parent = {
    posted: [],
    postMessage(message) { this.posted.push(message); }
  };
  const listeners = new Map();
  const calls = { materializations: 0, mounts: 0, unmounts: 0 };
  const active = new Set();
  const artifactVerifier = {
    async verifyAndMaterialize(
      _environment, policy, binding, artifactHTML, materialize,
      _defaultDigestText, isCurrent
    ) {
      const artifactDigest = await digestText(artifactHTML);
      if (artifactDigest !== binding.artifactDigest) {
        return Object.freeze({ status: "digest-mismatch" });
      }
      if (!isCurrent()) return Object.freeze({ status: "stale" });
      const materializedHTML = materialize();
      if (!policyModule.acceptsMaterializedHTMLBytes(policy, materializedHTML)) {
        return Object.freeze({ status: "materialization-refused" });
      }
      const materializedDigest = await digestText(materializedHTML);
      if (!isCurrent()) return Object.freeze({ status: "stale" });
      if (materializedDigest !== policy.materializedDigest) {
        return Object.freeze({ status: "materialization-refused" });
      }
      return Object.freeze({
        status: "verified", materializedHTML, materializedDigest,
        verificationReceipt: Object.freeze({})
      });
    }
  };
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
    createSurfaceHost(_environment, _primitives, options) {
      assert.equal(options.artifactPolicy.elevated, true);
      return {
        mount(surfaceId) {
          calls.mounts += 1;
          active.add(surfaceId);
          return true;
        },
        receive() { return false; },
        unmount(surfaceId) {
          calls.unmounts += 1;
          return active.delete(surfaceId);
        },
        dispose() { active.clear(); }
      };
    }
  };
  const environment = {
    parent,
    document: { getElementById() { return {}; } },
    addEventListener(type, listener) { listeners.set(type, listener); },
    removeEventListener(type) { listeners.delete(type); }
  };
  const bridge = createEmbeddingBridge(environment, {
    artifactPolicy,
    artifactVerifier,
    digestText,
    hostModule,
    primitives
  });
  return { bridge, calls, listeners, parent };
}

function mountRequest(artifactHTML, session = "session-large", surfaceId = "surface-a") {
  return {
    type: "nmp.outer.mount",
    requestId: `mount-${session}`,
    surfaceId,
    configuration: {
      session,
      artifactHTML,
      artifactBaseURL:
        "nmp-artifact://00000000-0000-4000-8000-000000000001/",
      domains: ["shell"],
      title: "Rage",
      binding: {
        manifestAuthor: "a".repeat(64),
        dTag: "clock",
        aggregateHash: "b".repeat(64),
        artifactDigest: digest(artifactHTML),
        surface: surfaceId,
        session
      }
    }
  };
}

async function dispatch(harness, data) {
  harness.listeners.get("message")({ source: harness.parent, data });
  await new Promise((resolve) => setImmediate(resolve));
}

test("exact elevated artifact is exclusive for one outer lifecycle", async () => {
  const exact = fixture();
  const harness = createHarness(exact.policy);
  await dispatch(harness, mountRequest(exact.artifactHTML));
  assert.equal(harness.calls.mounts, 1);
  assert.equal(harness.bridge.stateCounts().elevatedActive, 1);
  assert.equal(
    harness.bridge.stateCounts().reservedArtifactHTMLBytes,
    exact.policy.exactArtifactHTMLBytes
  );
  await dispatch(harness, mountRequest(
    exact.artifactHTML,
    "session-second",
    "surface-second"
  ));
  assert.equal(harness.calls.mounts, 1);
  assert.equal(harness.parent.posted.at(-1).error, "overloaded");
  await dispatch(harness, {
    type: "nmp.outer.unmount",
    requestId: "unmount-large",
    surfaceId: "surface-a",
    session: "session-large"
  });
  assert.equal(harness.bridge.stateCounts().elevatedActive, 0);
  await dispatch(harness, mountRequest(exact.artifactHTML, "session-remount"));
  assert.equal(harness.calls.mounts, 1);
  assert.equal(harness.parent.posted.at(-1).error, "overloaded");
});

test("mutation, mount-selected policy, and materialized mismatch never reach srcdoc", async () => {
  const exact = fixture();
  const mutatedHarness = createHarness(exact.policy);
  const mutated = mountRequest(`y${exact.artifactHTML.slice(1)}`);
  const posted = mutatedHarness.parent.posted.length;
  await dispatch(mutatedHarness, mutated);
  assert.equal(mutatedHarness.parent.posted.length, posted);
  assert.equal(mutatedHarness.calls.materializations, 0);
  assert.equal(mutatedHarness.calls.mounts, 0);

  const selected = mountRequest(exact.artifactHTML);
  selected.configuration.artifactPolicy = exact.policy;
  await dispatch(mutatedHarness, selected);
  assert.equal(mutatedHarness.calls.materializations, 0);
  assert.equal(mutatedHarness.calls.mounts, 0);

  const refusedHarness = createHarness({
    ...exact.policy,
    materializedDigest: "0".repeat(64)
  });
  await dispatch(refusedHarness, mountRequest(exact.artifactHTML));
  assert.equal(refusedHarness.calls.materializations, 1);
  assert.equal(refusedHarness.calls.mounts, 0);
  assert.equal(refusedHarness.parent.posted.at(-1).error, "materialization-refused");

  let digestCalls = 0;
  const oversizedHarness = createHarness({
    ...exact.policy,
    exactMaterializedHTMLBytes: exact.policy.exactMaterializedHTMLBytes - 1
  }, async (value) => {
    digestCalls += 1;
    return digest(value);
  });
  await dispatch(oversizedHarness, mountRequest(exact.artifactHTML));
  assert.equal(digestCalls, 1);
  assert.equal(oversizedHarness.calls.materializations, 1);
  assert.equal(oversizedHarness.calls.mounts, 0);
});

test("cancelled elevated digest holds reservation until settlement", async () => {
  const exact = fixture();
  let releaseDigest;
  const harness = createHarness(exact.policy, (value) => {
    if (value === exact.artifactHTML) {
      return new Promise((resolve) => {
        releaseDigest = () => resolve(digest(value));
      });
    }
    return Promise.resolve(digest(value));
  });
  harness.listeners.get("message")({
    source: harness.parent,
    data: mountRequest(exact.artifactHTML, "session-pending")
  });
  assert.equal(harness.bridge.stateCounts().elevatedPending, 1);
  await dispatch(harness, {
    type: "nmp.outer.unmount",
    requestId: "cancel-pending",
    surfaceId: "surface-a",
    session: "session-pending"
  });
  assert.equal(harness.bridge.stateCounts().elevatedPending, 1);
  releaseDigest();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.bridge.stateCounts().elevatedPending, 0);
  assert.equal(harness.calls.materializations, 0);
  assert.equal(harness.calls.mounts, 0);
});

test("page teardown invalidates authority but retains in-flight reservation", async () => {
  const exact = fixture();
  let releaseDigest;
  const harness = createHarness(exact.policy, (value) => {
    if (value === exact.artifactHTML) {
      return new Promise((resolve) => {
        releaseDigest = () => resolve(digest(value));
      });
    }
    return Promise.resolve(digest(value));
  });
  harness.listeners.get("message")({
    source: harness.parent,
    data: mountRequest(exact.artifactHTML, "session-pagehide")
  });
  const pagehide = harness.listeners.get("pagehide");
  pagehide();
  assert.equal(harness.listeners.has("message"), false);
  assert.equal(harness.bridge.stateCounts().elevatedPending, 1);
  releaseDigest();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.bridge.stateCounts().elevatedPending, 0);
  assert.equal(harness.calls.materializations, 0);
  assert.equal(harness.calls.mounts, 0);
});
