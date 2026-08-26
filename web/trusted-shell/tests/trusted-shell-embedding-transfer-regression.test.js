"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const {
  MAX_PARENT_MESSAGES_PER_SECOND
} = require("../trusted-shell-embedding.js");
const {
  ARTIFACT_CHUNK_BYTES,
  TRANSFER_DEADLINE_MS,
  createTransferManager
} = require("../trusted-shell-embedding-transfer.js");
const policyModule = require("../trusted-shell-artifact-policy.js");
const {
  chunkedMountRequests,
  createHarness,
  dispatch,
  mountRequest
} = require("./trusted-shell-embedding-test-harness.js");

test("expected chunks beyond the parent message window remain finite", async () => {
  const artifactBytes = ARTIFACT_CHUNK_BYTES *
    (MAX_PARENT_MESSAGES_PER_SECOND + 1) + 1;
  const artifactDigest = "c".repeat(64);
  const artifactPolicy = {
    maximumArtifactHTMLBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
    exactArtifactHTMLBytes: artifactBytes,
    maximumMaterializedHTMLBytes:
      policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES,
    exactMaterializedHTMLBytes: 1,
    artifactDigest,
    materializedDigest: "d".repeat(64),
    manifestAuthor: "a".repeat(64),
    dTag: "clock",
    aggregateHash: "b".repeat(64),
    exclusive: true
  };
  const harness = createHarness({ artifactPolicy });
  const transferId = "begin-burst";
  await dispatch(harness, {
    type: "nmp.outer.mount.begin",
    requestId: transferId,
    surfaceId: "surface-a",
    configuration: {
      session: "burst-session",
      artifactBytes,
      artifactBaseURL:
        "nmp-artifact://00000000-0000-4000-8000-000000000001/",
      domains: ["shell"],
      title: "Burst",
      binding: {
        manifestAuthor: "a".repeat(64),
        dTag: "clock",
        aggregateHash: "b".repeat(64),
        artifactDigest,
        surface: "surface-a",
        session: "burst-session"
      }
    }
  });
  const chunk = new ArrayBuffer(ARTIFACT_CHUNK_BYTES);
  for (let index = 0; index <= MAX_PARENT_MESSAGES_PER_SECOND; index += 1) {
    await dispatch(harness, {
      type: "nmp.outer.mount.chunk",
      requestId: `chunk-${index}`,
      surfaceId: "surface-a",
      session: "burst-session",
      transferId,
      offset: index * ARTIFACT_CHUNK_BYTES,
      bytes: chunk
    });
  }
  await dispatch(harness, {
    type: "nmp.outer.mount.chunk",
    requestId: "chunk-invalid-final",
    surfaceId: "surface-a",
    session: "burst-session",
    transferId,
    offset: ARTIFACT_CHUNK_BYTES * (MAX_PARENT_MESSAGES_PER_SECOND + 1),
    bytes: new ArrayBuffer(2)
  });
  assert.equal(harness.parent.posted.at(-1).message.error, "transfer-refused");
  assert.equal(harness.bridge.stateCounts().pendingTransfers, 0);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);
  assert.equal(harness.parent.posted.some(({ message }) =>
    message.type === "nmp.outer.rate-limited"), false);
  harness.listeners.get("pagehide")();
});

test("full-cap transfer replay preserves old bytes without allocating", () => {
  const allocations = [];
  const results = [];
  class FakeBytes {
    constructor(length) {
      this.byteLength = length;
      allocations.push(length);
    }
    set() {}
  }
  const manager = createTransferManager({
    ArrayBuffer,
    TextDecoder,
    Uint8Array: FakeBytes,
    clearTimeout() {},
    setTimeout() { return 1; }
  }, {
    artifactPolicy: policyModule.normalizeArtifactPolicy(),
    beforeBegin() {},
    contract: {
      snapshotDomains() { return Object.freeze(["shell"]); },
      validMountBegin() { return true; }
    },
    onComplete() {},
    primitives: {},
    reserveAdmission() { return null; },
    result(_type, _request, ok, error) { results.push({ ok, error }); },
    settleAdmission() {}
  });
  const request = {
    type: "nmp.outer.mount.begin",
    requestId: "full-cap",
    surfaceId: "surface-a",
    configuration: {
      artifactBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
      artifactBaseURL: "nmp-artifact://session/",
      binding: Object.freeze({}),
      domains: ["shell"],
      session: "full-cap-session",
      title: "Full cap"
    }
  };
  manager.handle(request);
  manager.handle(request);
  assert.deepEqual(allocations, [policyModule.HARD_MAX_ARTIFACT_HTML_BYTES]);
  assert.deepEqual(results, [
    { ok: true, error: null },
    { ok: false, error: "overloaded" }
  ]);
  assert.deepEqual(manager.counts(), {
    pendingTransfers: 1,
    reservedTransferBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES
  });
  manager.dispose();
});

test("chunk reconstruction preserves BOM and split multibyte bytes", async () => {
  const harness = createHarness();
  const artifactHTML = `\uFEFF${"a".repeat(ARTIFACT_CHUNK_BYTES - 4)}€終`;
  const original = new TextEncoder().encode(artifactHTML);
  for (const request of chunkedMountRequests(
    harness, "utf8-boundary", artifactHTML
  )) await dispatch(harness, request);
  const reconstructed = harness.calls.mounts[0].configuration.artifactHTML;
  assert.equal(reconstructed, artifactHTML);
  assert.deepEqual(new TextEncoder().encode(reconstructed), original);
});

test("failed transfer admission preserves the current surface", async () => {
  const harness = createHarness();
  await dispatch(harness, mountRequest(harness, "current-session"));
  for (let index = 0; index < 16; index += 1) {
    const requests = chunkedMountRequests(
      harness, `reserved-${index}`, "x", `reserved-surface-${index}`
    );
    await dispatch(harness, requests[0]);
  }
  const unmounts = harness.calls.unmounts.length;
  await dispatch(harness,
    chunkedMountRequests(harness, "replacement", "x")[0]);
  assert.equal(harness.parent.posted.at(-1).message.error, "overloaded");
  assert.equal(harness.calls.unmounts.length, unmounts);
  await dispatch(harness, {
    type: "nmp.outer.deliver",
    requestId: "deliver-current",
    surfaceId: "surface-a",
    session: "current-session",
    envelope: { type: "identity.changed" }
  });
  assert.equal(harness.parent.posted.at(-1).message.ok, true);
  harness.listeners.get("pagehide")();

  const allocationHarness = createHarness({
    allocateTransferBytes() { throw new RangeError("allocation refused"); }
  });
  await dispatch(allocationHarness,
    mountRequest(allocationHarness, "allocation-current"));
  const allocationUnmounts = allocationHarness.calls.unmounts.length;
  await dispatch(allocationHarness, chunkedMountRequests(
    allocationHarness, "allocation-replacement", "x"
  )[0]);
  assert.equal(allocationHarness.parent.posted.at(-1).message.error, "overloaded");
  assert.equal(allocationHarness.calls.unmounts.length, allocationUnmounts);
  assert.equal(allocationHarness.bridge.stateCounts().bindings, 1);
  allocationHarness.listeners.get("pagehide")();
});

test("abandoned transfer expires and releases its reservation", async () => {
  let timer;
  const harness = createHarness({
    clearTransferTimeout(id) { if (timer && timer.id === id) timer.cleared = true; },
    setTransferTimeout(callback, milliseconds) {
      timer = { callback, cleared: false, id: 7, milliseconds };
      return timer.id;
    }
  });
  await dispatch(harness,
    chunkedMountRequests(harness, "abandoned", "x")[0]);
  assert.equal(timer.milliseconds, TRANSFER_DEADLINE_MS);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 1);
  timer.callback();
  assert.equal(timer.cleared, true);
  assert.equal(harness.parent.posted.at(-1).message.error, "transfer-expired");
  assert.equal(harness.bridge.stateCounts().pendingTransfers, 0);
  assert.equal(harness.bridge.stateCounts().reservedTransferBytes, 0);
  assert.equal(harness.calls.mounts.length, 0);
  harness.listeners.get("pagehide")();
});

test("accepted chunks acknowledge progress and renew the finite deadline", async () => {
  const timers = [];
  const harness = createHarness({
    clearTransferTimeout(id) {
      const timer = timers.find((candidate) => candidate.id === id);
      if (timer) timer.cleared = true;
    },
    setTransferTimeout(callback, milliseconds) {
      const timer = {
        callback,
        cleared: false,
        id: timers.length + 1,
        milliseconds
      };
      timers.push(timer);
      return timer.id;
    }
  });
  const [begin, chunk] = chunkedMountRequests(harness, "progress", "x");
  await dispatch(harness, begin);
  assert.equal(timers.length, 1);
  await dispatch(harness, chunk);
  assert.equal(timers[0].cleared, true);
  assert.equal(timers.length, 2);
  assert.equal(timers[1].milliseconds, TRANSFER_DEADLINE_MS);
  assert.deepEqual(harness.parent.posted.at(-1).message, {
    type: "nmp.outer.mount.chunk.result",
    requestId: chunk.requestId,
    surfaceId: chunk.surfaceId,
    session: chunk.session,
    ok: true,
    error: null,
    binding: null
  });
  timers[1].callback();
  assert.equal(harness.parent.posted.at(-1).message.error, "transfer-expired");
  assert.equal(harness.bridge.stateCounts().pendingTransfers, 0);
  harness.listeners.get("pagehide")();
});
