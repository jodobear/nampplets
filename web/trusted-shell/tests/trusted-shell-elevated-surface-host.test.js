"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const policyModule = require("../trusted-shell-artifact-policy.js");
const { createSurfaceHost } = require("../trusted-shell-surface-host.js");
const NO_ENVELOPE = null;

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function createEnvironment() {
  const listeners = new Map();
  return {
    document: {
      createElement() {
        return {
          attributes: {},
          contentWindow: { postMessage() {} },
          setAttribute(name, value) { this.attributes[name] = value; },
          addEventListener() {},
          remove() { this.removed = true; }
        };
      }
    },
    addEventListener(type, listener) { listeners.set(type, listener); },
    removeEventListener(type) { listeners.delete(type); }
  };
}

function createFixture() {
  const artifactHTML = "x".repeat(
    policyModule.DEFAULT_MAX_ARTIFACT_HTML_BYTES + 1
  );
  const materializedHTML = `${artifactHTML}m`;
  return {
    artifactHTML,
    materializedHTML,
    policy: policyModule.normalizeArtifactPolicy({
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
    })
  };
}

function primitives() {
  return {
    isPlainObject(value) {
      return value !== null && typeof value === "object" && !Array.isArray(value);
    },
    isVerifiedArtifactBaseURL(value) { return value === "nmp-artifact://verified/"; },
    mappedEnvelope() { return NO_ENVELOPE; },
    materialize() { throw new Error("pre-materialized input required"); },
    projectNativeEnvelope() { return NO_ENVELOPE; }
  };
}

function surface() {
  return { replaceChildren(frame) { this.frame = frame; } };
}

test("surface host enforces the same normalized elevated policy at srcdoc", () => {
  const fixture = createFixture();
  const target = surface();
  const host = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  assert.equal(host.mount("rage", target, {
    session: "rage-session",
    artifactHTML: fixture.artifactHTML,
    artifactDigest: fixture.policy.artifactDigest,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest,
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"],
    title: "Rage"
  }), true);
  assert.equal(target.frame.srcdoc, fixture.materializedHTML);
  assert.deepEqual(target.frame.attributes, {
    sandbox: "allow-scripts",
    referrerpolicy: "no-referrer",
    "aria-label": "Rage"
  });
});

test("surface host refuses elevated byte or digest drift before srcdoc", () => {
  const fixture = createFixture();
  const host = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const sameLengthMutation = `y${fixture.artifactHTML.slice(1)}`;
  for (const configuration of [{
    artifactHTML: `${fixture.artifactHTML}x`,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest
  }, {
    artifactHTML: sameLengthMutation,
    artifactDigest: digest(sameLengthMutation),
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest
  }, {
    artifactHTML: fixture.artifactHTML,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: "0".repeat(64)
  }]) {
    const target = surface();
    assert.equal(host.mount("rage", target, {
      session: "rage-session",
      artifactDigest: fixture.policy.artifactDigest,
      artifactBaseURL: "nmp-artifact://verified/",
      domains: ["shell"],
      title: "Rage",
      ...configuration
    }), false);
    assert.equal(target.frame, undefined);
  }
});

test("surface host requires preverified elevated materialization", () => {
  const fixture = createFixture();
  let materializations = 0;
  const suppliedPrimitives = primitives();
  suppliedPrimitives.materialize = () => {
    materializations += 1;
    return fixture.materializedHTML;
  };
  const target = surface();
  const host = createSurfaceHost(createEnvironment(), suppliedPrimitives, {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  assert.equal(host.mount("rage", target, {
    session: "rage-session",
    artifactHTML: fixture.artifactHTML,
    artifactDigest: fixture.policy.artifactDigest,
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"]
  }), false);
  assert.equal(materializations, 0);
  assert.equal(target.frame, undefined);
});

test("surface host elevated admission cannot remount after teardown", () => {
  const fixture = createFixture();
  const host = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const configuration = {
    session: "rage-session",
    artifactHTML: fixture.artifactHTML,
    artifactDigest: fixture.policy.artifactDigest,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest,
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"]
  };
  assert.equal(host.mount("rage", surface(), configuration), true);
  assert.equal(host.unmount("rage"), true);
  assert.equal(host.mount("rage", surface(), configuration), false);
});

test("surface host refuses unnormalized policy lookalikes", () => {
  const fixture = createFixture();
  assert.throws(() => createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: { ...fixture.policy }
  }), /must be normalized/);
});
