"use strict";

const crypto = require("node:crypto");
const { createEmbeddingBridge } = require("../trusted-shell-embedding.js");
const {
  ARTIFACT_CHUNK_BYTES
} = require("../trusted-shell-embedding-transfer.js");

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
    createSurfaceHost(_environment, _primitives, hostOptions) {
      forwardEnvelope = hostOptions.forwardEnvelope;
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
    allocateTransferBytes: options.allocateTransferBytes,
    artifactPolicy: options.artifactPolicy,
    clearTransferTimeout: options.clearTransferTimeout,
    primitives,
    hostModule,
    digestText: options.digestText || (async (value) => digest(value)),
    now: () => currentTime,
    setTransferTimeout: options.setTransferTimeout
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

module.exports = Object.freeze({
  chunkedMountRequests,
  createHarness,
  digest,
  dispatch,
  mountRequest
});
