"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const policyModule = require("../trusted-shell-artifact-policy.js");
const {
  renderBootstrap,
  renderEmbeddedShell
} = require("../scripts/render-trusted-shell-embedded.js");

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function elevatedPolicy(overrides = {}) {
  const artifact = overrides.artifact || "x".repeat(
    policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES + 1
  );
  const materialized = overrides.materialized || `${artifact}m`;
  const input = {
    maximumArtifactHTMLBytes: policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
    exactArtifactHTMLBytes: Buffer.byteLength(artifact),
    maximumMaterializedHTMLBytes:
      policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES,
    exactMaterializedHTMLBytes: Buffer.byteLength(materialized),
    artifactDigest: digest(artifact),
    materializedDigest: digest(materialized),
    manifestAuthor: "a".repeat(64),
    dTag: "rage-single-player",
    aggregateHash: "b".repeat(64),
    exclusive: true,
    ...overrides.input
  };
  return { artifact, input, materialized };
}

test("default policy preserves the existing byte ceilings", () => {
  const policy = policyModule.normalizeArtifactPolicy();
  assert.equal(policy.elevated, false);
  assert.equal(
    policy.maximumArtifactHTMLBytes,
    policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES
  );
  assert.equal(
    policy.maximumMaterializedHTMLBytes,
    policyModule.DEFAULT_MAX_MATERIALIZED_HTML_BYTES
  );
  assert.equal(policyModule.constructorInput(policy), null);
  assert.equal(policyModule.acceptsArtifactHTML(
    policy,
    "x".repeat(policy.maximumArtifactHTMLBytes)
  ), true);
  assert.equal(policyModule.acceptsArtifactHTML(
    policy,
    "x".repeat(policy.maximumArtifactHTMLBytes + 1)
  ), false);
  assert.equal(policyModule.acceptsArtifactHTML(policy, ""), true);
  assert.equal(policyModule.acceptsArtifactHTMLBytes(policy, 0), false);
});

test("artifact byte admission is positive, finite, and policy-bound", () => {
  const policy = policyModule.normalizeArtifactPolicy();
  assert.equal(policyModule.acceptsArtifactHTMLBytes(policy, 1), true);
  assert.equal(policyModule.acceptsArtifactHTMLBytes(
    policy, policy.maximumArtifactHTMLBytes
  ), true);
  for (const value of [
    0, -1, policy.maximumArtifactHTMLBytes + 1,
    Number.MAX_SAFE_INTEGER + 1, 1.5
  ]) {
    assert.equal(policyModule.acceptsArtifactHTMLBytes(policy, value), false);
  }
});

test("elevated policy is exact-artifact-bound under immutable hard ceilings", () => {
  const fixture = elevatedPolicy();
  const policy = policyModule.normalizeArtifactPolicy(fixture.input);
  assert.equal(
    policyModule.HARD_MAX_ARTIFACT_HTML_BYTES,
    120 * 1024 * 1024
  );
  assert.equal(
    policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES,
    120 * 1024 * 1024
  );
  assert.equal(
    policy.maximumArtifactHTMLBytes,
    policyModule.HARD_MAX_ARTIFACT_HTML_BYTES
  );
  assert.equal(
    policy.maximumMaterializedHTMLBytes,
    policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES
  );
  assert.equal(Object.isFrozen(policy), true);
  assert.equal(policyModule.isNormalizedPolicy(policy), true);
  assert.deepEqual(
    policyModule.constructorInput(policy),
    fixture.input
  );
  assert.equal(policyModule.acceptsArtifactHTML(policy, fixture.artifact), true);
  assert.equal(policyModule.acceptsArtifactHTMLBytes(
    policy, fixture.input.exactArtifactHTMLBytes
  ), true);
  assert.equal(policyModule.acceptsArtifactHTMLBytes(
    policy, fixture.input.exactArtifactHTMLBytes - 1
  ), false);
  assert.equal(policyModule.acceptsArtifactHTMLBytes(
    policy, fixture.input.exactArtifactHTMLBytes + 1
  ), false);
  assert.equal(
    policyModule.acceptsArtifactHTML(policy, `${fixture.artifact}y`),
    false
  );
  assert.equal(policyModule.acceptsMaterializedHTML(
    policy,
    fixture.materialized,
    digest(fixture.materialized)
  ), true);
  assert.equal(policyModule.acceptsMaterializedHTML(
    policy,
    fixture.materialized,
    "0".repeat(64)
  ), false);
  assert.equal(policyModule.matchesBinding(policy, {
    artifactDigest: fixture.input.artifactDigest,
    manifestAuthor: fixture.input.manifestAuthor,
    dTag: fixture.input.dTag,
    aggregateHash: fixture.input.aggregateHash
  }), true);
});

test("invalid elevated inputs fail closed without clamping", () => {
  const { input } = elevatedPolicy();
  const invalidValues = [
    { maximumArtifactHTMLBytes: 0 },
    { maximumArtifactHTMLBytes: 1.5 },
    { maximumArtifactHTMLBytes: "100663296" },
    { maximumArtifactHTMLBytes: Number.NaN },
    { maximumArtifactHTMLBytes: Number.POSITIVE_INFINITY },
    { maximumArtifactHTMLBytes:
      policyModule.HARD_MAX_ARTIFACT_HTML_BYTES + 1 },
    { maximumMaterializedHTMLBytes:
      policyModule.HARD_MAX_MATERIALIZED_HTML_BYTES + 1 },
    { exactArtifactHTMLBytes: policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES },
    { exactMaterializedHTMLBytes: 0 },
    { artifactDigest: "0".repeat(63) },
    { artifactDigest: { toString: () => "0".repeat(64) } },
    { materializedDigest: "g".repeat(64) },
    { manifestAuthor: "a" },
    { dTag: "bad\u0000tag" },
    { aggregateHash: [] },
    { exclusive: false }
  ];
  for (const override of invalidValues) {
    assert.throws(
      () => policyModule.normalizeArtifactPolicy({ ...input, ...override }),
      /invalid trusted artifact policy/
    );
  }
  assert.throws(
    () => policyModule.normalizeArtifactPolicy({ ...input, extra: true }),
    /invalid trusted artifact policy/
  );
  const confused = Object.assign(Object.create({ inherited: true }), input);
  assert.throws(
    () => policyModule.normalizeArtifactPolicy(confused),
    /invalid trusted artifact policy/
  );
  let reads = 0;
  const accessorInput = { ...input };
  Object.defineProperty(accessorInput, "exactArtifactHTMLBytes", {
    enumerable: true,
    get() {
      reads += 1;
      return input.exactArtifactHTMLBytes;
    }
  });
  assert.throws(
    () => policyModule.normalizeArtifactPolicy(accessorInput),
    /invalid trusted artifact policy/
  );
  assert.equal(reads, 0);
});

test("UTF-8 measurement matches TextEncoder without allocating through it", () => {
  const values = [
    "", "ascii", "\u0080", "\u07ff", "\u0800", "😀", "a😀z",
    "\ud800", "\udc00", "\ud800x", "\ud800\udc00", "日本語"
  ];
  for (const value of values) {
    assert.equal(
      policyModule.utf8ByteLength(value),
      new TextEncoder().encode(value).byteLength,
      JSON.stringify(value)
    );
  }
  assert.equal(policyModule.utf8ByteLength(null), null);
});

test("elevated admission reserves once and never releases pending work early", () => {
  const fixture = elevatedPolicy();
  const policy = policyModule.normalizeArtifactPolicy(fixture.input);
  const admission = policyModule.createAdmission(policy);
  const token = admission.begin();
  assert.ok(token);
  assert.equal(admission.begin(), false);
  assert.deepEqual(admission.counts(), {
    elevatedPending: 1,
    elevatedActive: 0,
    reservedArtifactHTMLBytes: fixture.input.exactArtifactHTMLBytes,
    reservedMaterializedHTMLBytes: fixture.input.exactMaterializedHTMLBytes
  });
  admission.dispose();
  assert.equal(admission.counts().elevatedPending, 1);
  admission.settle(token);
  assert.deepEqual(admission.counts(), {
    elevatedPending: 0,
    elevatedActive: 0,
    reservedArtifactHTMLBytes: 0,
    reservedMaterializedHTMLBytes: 0
  });
  assert.equal(admission.begin(), false);
});

test("active elevated admission remains exclusive until teardown", () => {
  const fixture = elevatedPolicy();
  const policy = policyModule.normalizeArtifactPolicy(fixture.input);
  const admission = policyModule.createAdmission(policy);
  const token = admission.begin();
  assert.equal(admission.activate(token), true);
  admission.settle(token);
  assert.equal(admission.counts().elevatedActive, 1);
  assert.equal(admission.begin(), false);
  admission.release(token);
  assert.equal(admission.counts().elevatedActive, 0);
  assert.equal(admission.begin(), false);
});

test("deterministic renderer owns the constructor input", () => {
  const fixture = elevatedPolicy();
  assert.match(
    renderBootstrap(),
    /createEmbeddingBridge\(window\);/
  );
  const bootstrap = renderBootstrap(fixture.input);
  assert.match(bootstrap, /createEmbeddingBridge\(window, \{ artifactPolicy:/);
  assert.match(bootstrap, new RegExp(fixture.input.artifactDigest));
  assert.equal(renderEmbeddedShell({ artifactPolicy: fixture.input }),
    renderEmbeddedShell({ artifactPolicy: fixture.input }));
  assert.throws(
    () => renderEmbeddedShell({ artifactPolicy: fixture.input, url: "forbidden" }),
    /unsupported embedded trusted-shell option/
  );
  const hostile = renderBootstrap({
    ...fixture.input,
    dTag: "</script><script>forbidden()"
  });
  assert.equal((hostile.match(/<\/script>/g) || []).length, 1);
  assert.doesNotMatch(hostile, /"dTag":"<|<script>forbidden/);
  assert.match(hostile, /\\u003c\/script\\u003e/);
});
