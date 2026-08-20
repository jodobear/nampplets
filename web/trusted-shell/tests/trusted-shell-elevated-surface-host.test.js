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
    crypto: crypto.webcrypto,
    TextEncoder,
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

function binding(fixture) {
  return Object.freeze({
    artifactDigest: fixture.policy.artifactDigest,
    manifestAuthor: fixture.policy.manifestAuthor,
    dTag: fixture.policy.dTag,
    aggregateHash: fixture.policy.aggregateHash
  });
}

async function verifiedReceipt(host, fixture, manifestBinding) {
  const verified = await host.verifyAndMaterialize(
    manifestBinding, fixture.artifactHTML,
    () => fixture.materializedHTML, undefined, () => true
  );
  assert.equal(verified.status, "verified");
  return verified.verificationReceipt;
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

test("surface host enforces the same normalized elevated policy at srcdoc", async () => {
  const fixture = createFixture();
  const environment = createEnvironment();
  const manifestBinding = binding(fixture);
  const target = surface();
  const host = createSurfaceHost(environment, primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const receipt = await verifiedReceipt(host, fixture, manifestBinding);
  assert.equal(host.mount("rage", target, {
    session: "rage-session",
    artifactHTML: fixture.artifactHTML,
    artifactDigest: fixture.policy.artifactDigest,
    binding: manifestBinding,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest,
    verificationReceipt: receipt,
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

test("surface host refuses elevated byte or digest drift before srcdoc", async () => {
  const fixture = createFixture();
  const sameLengthMutation = `y${fixture.artifactHTML.slice(1)}`;
  const materializedMutation = `y${fixture.materializedHTML.slice(1)}`;
  for (const configuration of [{
    artifactHTML: `${fixture.artifactHTML}x`,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest
  }, {
    artifactHTML: sameLengthMutation,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest
  }, {
    artifactHTML: fixture.artifactHTML,
    materializedHTML: materializedMutation,
    materializedDigest: fixture.policy.materializedDigest
  }, {
    artifactHTML: fixture.artifactHTML,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: "0".repeat(64)
  }]) {
    const environment = createEnvironment();
    const manifestBinding = binding(fixture);
    const host = createSurfaceHost(environment, primitives(), {
      acceptMaterializedHTML: true,
      artifactPolicy: fixture.policy
    });
    const receipt = await verifiedReceipt(host, fixture, manifestBinding);
    const target = surface();
    assert.equal(host.mount("rage", target, {
      session: "rage-session",
      artifactDigest: fixture.policy.artifactDigest,
      binding: manifestBinding,
      artifactBaseURL: "nmp-artifact://verified/",
      domains: ["shell"],
      title: "Rage",
      verificationReceipt: receipt,
      ...configuration
    }), false);
    assert.equal(target.frame, undefined);
  }
});

test("surface host requires cryptographically verified materialization", () => {
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
    binding: binding(fixture),
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"]
  }), false);
  assert.equal(materializations, 0);
  assert.equal(target.frame, undefined);
});

test("surface host receipt and admission cannot replay after teardown", async () => {
  const fixture = createFixture();
  const environment = createEnvironment();
  const manifestBinding = binding(fixture);
  const host = createSurfaceHost(environment, primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const receipt = await verifiedReceipt(host, fixture, manifestBinding);
  const configuration = {
    session: "rage-session",
    artifactHTML: fixture.artifactHTML,
    artifactDigest: fixture.policy.artifactDigest,
    binding: manifestBinding,
    materializedHTML: fixture.materializedHTML,
    materializedDigest: fixture.policy.materializedDigest,
    verificationReceipt: receipt,
    artifactBaseURL: "nmp-artifact://verified/",
    domains: ["shell"]
  };
  assert.equal(host.mount("rage", surface(), configuration), true);
  assert.equal(host.unmount("rage"), true);
  assert.equal(host.mount("rage", surface(), configuration), false);
  const secondHost = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  assert.equal(secondHost.mount("rage", surface(), configuration), false);

  const disposeEnvironment = createEnvironment();
  const disposeBinding = binding(fixture);
  const disposeHost = createSurfaceHost(disposeEnvironment, primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const disposeReceipt = await verifiedReceipt(
    disposeHost, fixture, disposeBinding
  );
  const disposeConfiguration = {
    ...configuration,
    binding: disposeBinding,
    verificationReceipt: disposeReceipt
  };
  assert.equal(disposeHost.mount("rage", surface(), disposeConfiguration), true);
  disposeHost.dispose();
  const afterDispose = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  assert.equal(afterDispose.mount("rage", surface(), disposeConfiguration), false);

  const changedPolicy = policyModule.normalizeArtifactPolicy({
    ...policyModule.constructorInput(fixture.policy),
    dTag: "different"
  });
  const policyEnvironment = createEnvironment();
  const policyBinding = binding(fixture);
  const policyHost = createSurfaceHost(policyEnvironment, primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const policyReceipt = await verifiedReceipt(policyHost, fixture, policyBinding);
  const changedHost = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: changedPolicy
  });
  assert.equal(changedHost.mount("rage", surface(), {
    ...configuration,
    binding: policyBinding,
    verificationReceipt: policyReceipt
  }), false);

  const mismatchEnvironment = createEnvironment();
  const mismatchBinding = binding(fixture);
  const mismatchHost = createSurfaceHost(mismatchEnvironment, primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  const mismatchReceipt = await verifiedReceipt(
    mismatchHost, fixture, mismatchBinding
  );
  const mismatchConfiguration = {
    ...configuration,
    artifactHTML: `y${fixture.artifactHTML.slice(1)}`,
    binding: mismatchBinding,
    verificationReceipt: mismatchReceipt
  };
  assert.equal(mismatchHost.mount("rage", surface(), mismatchConfiguration), false);
  const mismatchReplay = createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: fixture.policy
  });
  assert.equal(mismatchReplay.mount("rage", surface(), {
    ...mismatchConfiguration,
    artifactHTML: fixture.artifactHTML
  }), false);
});

test("surface host refuses unnormalized policy lookalikes", () => {
  const fixture = createFixture();
  assert.throws(() => createSurfaceHost(createEnvironment(), primitives(), {
    acceptMaterializedHTML: true,
    artifactPolicy: { ...fixture.policy }
  }), /must be normalized/);
});
