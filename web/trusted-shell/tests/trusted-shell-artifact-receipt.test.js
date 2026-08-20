"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const policyModule = require("../trusted-shell-artifact-policy.js");
const { createSurfaceHost } = require("../trusted-shell-surface-host.js");

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function fixture() {
  const artifactHTML = "x".repeat(
    policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES + 1
  );
  const materializedHTML = `${artifactHTML}m`;
  const policy = policyModule.normalizeArtifactPolicy({
    maximumArtifactHTMLBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
    exactArtifactHTMLBytes: Buffer.byteLength(artifactHTML),
    maximumMaterializedHTMLBytes:
      policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES,
    exactMaterializedHTMLBytes: Buffer.byteLength(materializedHTML),
    artifactDigest: digest(artifactHTML),
    materializedDigest: digest(materializedHTML),
    manifestAuthor: "a".repeat(64),
    dTag: "rage",
    aggregateHash: "b".repeat(64),
    exclusive: true
  });
  return { artifactHTML, materializedHTML, policy };
}

function environment() {
  return {
    crypto: crypto.webcrypto,
    TextEncoder,
    document: {
      createElement() {
        return {
          contentWindow: { postMessage() {} },
          setAttribute() {},
          addEventListener() {},
          remove() {}
        };
      }
    },
    addEventListener() {},
    removeEventListener() {}
  };
}

function primitives() {
  return {
    isPlainObject(value) {
      return value !== null && typeof value === "object" && !Array.isArray(value);
    },
    isVerifiedArtifactBaseURL(value) {
      return value === "nmp-artifact://verified/";
    },
    mappedEnvelope() { return null; },
    projectNativeEnvelope() { return null; }
  };
}

function host(exact) {
  return createSurfaceHost(environment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: exact.policy
  });
}

function binding(exact) {
  return Object.freeze({
    artifactDigest: exact.policy.artifactDigest,
    manifestAuthor: exact.policy.manifestAuthor,
    dTag: exact.policy.dTag,
    aggregateHash: exact.policy.aggregateHash
  });
}

async function receipt(issuingHost, exact, manifestBinding) {
  const verified = await issuingHost.verifyAndMaterialize(
    manifestBinding,
    exact.artifactHTML,
    () => exact.materializedHTML,
    undefined,
    () => true
  );
  assert.equal(verified.status, "verified");
  return verified.verificationReceipt;
}

function configuration(exact, manifestBinding, verificationReceipt) {
  return {
    session: "rage-session",
    artifactHTML: exact.artifactHTML,
    binding: manifestBinding,
    materializedHTML: exact.materializedHTML,
    materializedDigest: exact.policy.materializedDigest,
    verificationReceipt,
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"]
  };
}

function surface() {
  return { replaceChildren(frame) { this.frame = frame; } };
}

test("receipt is bound to issuing host and current lifecycle", async () => {
  const exact = fixture();
  const manifestBinding = binding(exact);
  const hostA = host(exact);
  const receiptA = await receipt(hostA, exact, manifestBinding);
  const configA = configuration(exact, manifestBinding, receiptA);
  hostA.dispose();
  const targetA = surface();
  assert.equal(hostA.mount("rage", targetA, configA), false);
  assert.equal(targetA.frame, undefined);
  const hostB = host(exact);
  const targetB = surface();
  assert.equal(hostB.mount("rage", targetB, configA), false);
  assert.equal(targetB.frame, undefined);

  const hostC = host(exact);
  const receiptC = await receipt(hostC, exact, manifestBinding);
  const configC = configuration(exact, manifestBinding, receiptC);
  assert.equal(hostC.invalidateArtifactVerification(), true);
  assert.equal(hostC.mount("rage", surface(), configC), false);
  assert.equal(host(exact).mount("rage", surface(), configC), false);
});

test("current issuing host consumes exact receipt once", async () => {
  const exact = fixture();
  const issuingHost = host(exact);
  const manifestBinding = binding(exact);
  const verified = await receipt(issuingHost, exact, manifestBinding);
  const config = configuration(exact, manifestBinding, verified);
  const target = surface();
  assert.equal(issuingHost.mount("rage", target, config), true);
  assert.equal(target.frame.srcdoc, exact.materializedHTML);
  issuingHost.unmount("rage");
  assert.equal(issuingHost.mount("rage", surface(), config), false);
});
