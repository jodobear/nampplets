(function trustedShellArtifactVerifierModule(global) {
  "use strict";

  const policySource = global.NMPTrustedShellArtifactPolicy ||
    (typeof require === "function"
      ? require("./trusted-shell-artifact-policy.js")
      : null);
  const NO_RECEIPT = null;
  const verifiedReceipts = new WeakMap();
  const consumedReceipts = new WeakSet();

  function result(status, values = {}) {
    return Object.freeze({ status, ...values });
  }

  async function verifyAndMaterialize(
    environment, policy, binding, artifactHTML, materialize,
    defaultDigestText, isCurrent
  ) {
    if (!policySource.isNormalizedPolicy(policy) ||
        !policySource.matchesBinding(policy, binding) ||
        !policySource.acceptsArtifactHTML(policy, artifactHTML) ||
        typeof materialize !== "function" || typeof isCurrent !== "function") {
      return result("digest-mismatch");
    }
    const digestText = policy.elevated
      ? (value) => policySource.digestText(environment, value)
      : defaultDigestText;
    if (typeof digestText !== "function") return result("digest-mismatch");
    const artifactDigest = await digestText(artifactHTML);
    if (artifactDigest !== binding.artifactDigest) {
      return result("digest-mismatch");
    }
    if (!isCurrent()) return result("stale");
    const materializedHTML = materialize();
    if (!policySource.acceptsMaterializedHTMLBytes(
      policy, materializedHTML
    )) return result("materialization-refused");
    const materializedDigest = await digestText(materializedHTML);
    if (!isCurrent()) return result("stale");
    if (!policySource.acceptsMaterializedHTML(
      policy, materializedHTML, materializedDigest
    )) return result("materialization-refused");
    let verificationReceipt = NO_RECEIPT;
    if (policy.elevated) {
      verificationReceipt = Object.freeze({});
      verifiedReceipts.set(verificationReceipt, Object.freeze({
        artifactHTML, binding, materializedHTML, policy
      }));
    }
    return result("verified", {
      artifactDigest, materializedDigest, materializedHTML,
      verificationReceipt
    });
  }

  function consumeArtifactReceipt(
    policy, receipt, binding, artifactHTML, materializedHTML
  ) {
    const verified = verifiedReceipts.get(receipt);
    if (!verified || consumedReceipts.has(receipt)) return false;
    consumedReceipts.add(receipt);
    return verified.policy === policy && verified.binding === binding &&
      verified.artifactHTML === artifactHTML &&
      verified.materializedHTML === materializedHTML;
  }

  function beginVerifiedMount(admission, policy, configuration) {
    const token = admission.begin();
    if (!policy.elevated) return token;
    if (!token || !consumeArtifactReceipt(
      policy, configuration.verificationReceipt, configuration.binding,
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
    verifyAndMaterialize
  });
  global.NMPTrustedShellArtifactVerifier = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
