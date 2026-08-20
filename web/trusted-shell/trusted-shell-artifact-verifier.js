(function trustedShellArtifactVerifierModule(global) {
  "use strict";

  const policySource = global.NMPTrustedShellArtifactPolicy ||
    (typeof require === "function"
      ? require("./trusted-shell-artifact-policy.js")
      : null);
  const NO_RECEIPT = null;
  const lifecycles = new WeakMap();
  const verifiedReceipts = new WeakMap();
  const consumedReceipts = new WeakSet();

  function result(status, values = {}) {
    return Object.freeze({ status, ...values });
  }

  function createArtifactLifecycle() {
    const lifecycle = Object.freeze({});
    lifecycles.set(lifecycle, { disposed: false, generation: 0 });
    return lifecycle;
  }

  function invalidateArtifactLifecycle(lifecycle, terminal = false) {
    const state = lifecycles.get(lifecycle);
    if (!state || state.disposed) return false;
    state.generation += 1;
    if (terminal) state.disposed = true;
    return true;
  }

  async function verifyAndMaterialize(
    lifecycle, environment, policy, binding, artifactHTML, materialize,
    defaultDigestText, isCurrent
  ) {
    const state = lifecycles.get(lifecycle);
    if (!state || state.disposed || !policySource.isNormalizedPolicy(policy) ||
        !policySource.matchesBinding(policy, binding) ||
        !policySource.acceptsArtifactHTML(policy, artifactHTML) ||
        typeof materialize !== "function" || typeof isCurrent !== "function") {
      return result("digest-mismatch");
    }
    const generation = state.generation;
    const current = () => !state.disposed && state.generation === generation &&
      isCurrent();
    const digestText = policy.elevated
      ? (value) => policySource.digestText(environment, value)
      : defaultDigestText;
    if (typeof digestText !== "function") return result("digest-mismatch");
    const artifactDigest = await digestText(artifactHTML);
    if (artifactDigest !== binding.artifactDigest) {
      return result("digest-mismatch");
    }
    if (!current()) return result("stale");
    const materializedHTML = materialize();
    if (!policySource.acceptsMaterializedHTMLBytes(
      policy, materializedHTML
    )) return result("materialization-refused");
    const materializedDigest = await digestText(materializedHTML);
    if (!current()) return result("stale");
    if (!policySource.acceptsMaterializedHTML(
      policy, materializedHTML, materializedDigest
    )) return result("materialization-refused");
    let verificationReceipt = NO_RECEIPT;
    if (policy.elevated) {
      verificationReceipt = Object.freeze({});
      verifiedReceipts.set(verificationReceipt, Object.freeze({
        artifactHTML, binding, generation, lifecycle, materializedHTML, policy
      }));
    }
    return result("verified", {
      artifactDigest, materializedDigest, materializedHTML,
      verificationReceipt
    });
  }

  function consumeArtifactReceipt(
    lifecycle, policy, receipt, binding, artifactHTML, materializedHTML
  ) {
    const verified = verifiedReceipts.get(receipt);
    if (!verified || consumedReceipts.has(receipt)) return false;
    consumedReceipts.add(receipt);
    const state = lifecycles.get(lifecycle);
    return Boolean(state && !state.disposed &&
      verified.lifecycle === lifecycle && verified.generation === state.generation &&
      verified.policy === policy && verified.binding === binding &&
      verified.artifactHTML === artifactHTML &&
      verified.materializedHTML === materializedHTML);
  }

  function beginVerifiedMount(lifecycle, admission, policy, configuration) {
    const token = admission.begin();
    if (!policy.elevated) return token;
    if (!token || !consumeArtifactReceipt(
      lifecycle, policy, configuration.verificationReceipt, configuration.binding,
      configuration.artifactHTML, configuration.materializedHTML
    )) {
      admission.settle(token);
      return false;
    }
    return token;
  }

  const exported = Object.freeze({
    beginVerifiedMount,
    consumeArtifactReceipt,
    createArtifactLifecycle,
    invalidateArtifactLifecycle,
    verifyAndMaterialize
  });
  global.NMPTrustedShellArtifactVerifier = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
