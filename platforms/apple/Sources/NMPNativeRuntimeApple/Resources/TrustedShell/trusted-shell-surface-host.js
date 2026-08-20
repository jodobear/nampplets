(function trustedShellSurfaceHost(global) {
  "use strict";
  const MAX_SURFACES = 16, MAX_SURFACE_ID_BYTES = 128;
  const MAX_SESSION_ID_BYTES = 256, MAX_TITLE_BYTES = 1024;
  const MAX_DOMAINS = 64, MAX_DOMAIN_BYTES = 64;
  const MAX_NAPPLET_MESSAGES_PER_SECOND = 256;
  const primitiveSource = global.NMPTrustedShellPrimitives ||
    (typeof require === "function" ? require("./trusted-shell.js") : null);
  const artifactPolicySource = global.NMPTrustedShellArtifactPolicy ||
    (typeof require === "function" ?
      require("./trusted-shell-artifact-policy.js") : null);
  const artifactVerifierSource = global.NMPTrustedShellArtifactVerifier || (typeof require === "function" ? require("./trusted-shell-artifact-verifier.js") : null);
  const MAX_ARTIFACT_HTML_BYTES = artifactPolicySource.DEFAULT_MAX_ARTIFACT_HTML_BYTES;
  const MAX_MATERIALIZED_HTML_BYTES = artifactPolicySource.DEFAULT_MAX_MATERIALIZED_HTML_BYTES;
  const REQUIRED_CONFIGURATION_FIELDS = Object.freeze(
    ["artifactBaseURL", "artifactHTML", "session"]);
  const OPTIONAL_CONFIGURATION_FIELDS = Object.freeze([
    "artifactDigest", "binding", "domains", "materializedDigest",
    "materializedHTML", "onError", "onReady", "title", "verificationReceipt"
  ]);
  function validText(environment, value, maximumBytes, allowEmpty = false) {
    return typeof value === "string" &&
      (allowEmpty || value.length > 0) &&
      artifactPolicySource.utf8ByteLength(value) <= maximumBytes &&
      !/[\u0000-\u001f\u007f]/.test(value);
  }
  function validDomains(environment, domains) {
    return typeof domains === "undefined" ||
      (Array.isArray(domains) &&
        domains.length <= MAX_DOMAINS &&
        domains.every((domain) =>
          validText(environment, domain, MAX_DOMAIN_BYTES) &&
          /^[a-z][a-z0-9-]*$/.test(domain)
        ));
  }
  function createSurfaceHost(environment, suppliedPrimitives, options = {}) {
    const primitives = suppliedPrimitives || primitiveSource;
    if (!primitives || !artifactVerifierSource ||
        !environment || !environment.document) {
      throw new Error("The trusted shell surface host is unavailable");
    }
    const artifactPolicy = typeof options.artifactPolicy === "undefined"
      ? artifactPolicySource.normalizeArtifactPolicy()
      : options.artifactPolicy;
    if (!artifactPolicySource.isNormalizedPolicy(artifactPolicy)) {
      throw new TypeError("trusted artifact policy must be normalized");
    }
    const forwardEnvelope = typeof options.forwardEnvelope === "function"
      ? options.forwardEnvelope
      : null;
    const acceptMaterializedHTML = options.acceptMaterializedHTML === true;
    const now = typeof options.now === "function" ? options.now : Date.now;
    const admission = artifactPolicySource.createAdmission(artifactPolicy);
    const artifactLifecycle = artifactVerifierSource.createArtifactLifecycle();
    const surfaces = new Map();
    let disposed = false;
    function closeAcknowledgement(state) {
      if (state.acknowledgement) {
        state.acknowledgement.close();
        state.acknowledgement = null;
      }
    }
    function forwardToNative(surfaceId, state, envelope) {
      if (forwardEnvelope) {
        forwardEnvelope(Object.freeze({
          surfaceId,
          session: state.session,
          envelope
        }));
        return;
      }
      const root = environment.document.documentElement;
      root.setAttribute("data-nmp-native-envelope", JSON.stringify({
        session: state.session,
        envelope
      }));
      environment.document.dispatchEvent(
        new environment.Event(primitives.bridgeEventName)
      );
      root.removeAttribute("data-nmp-native-envelope");
    }
    function receiveNappletMessage(event) {
      for (const [surfaceId, state] of surfaces.entries()) {
        const envelope = primitives.mappedEnvelope(event, state.frame);
        if (envelope !== null) {
          const currentTime = now();
          if (currentTime - state.messageWindowStartedAt >= 1000) {
            state.messageWindowStartedAt = currentTime;
            state.messagesInWindow = 0;
          }
          if (state.messagesInWindow >= MAX_NAPPLET_MESSAGES_PER_SECOND) {
            const onError = state.onError;
            unmount(surfaceId);
            try {
              if (onError) onError(surfaceId, "message rate exceeded");
            } catch (_) {}
            return;
          }
          state.messagesInWindow += 1;
          forwardToNative(surfaceId, state, envelope);
          return;
        }
      }
    }
    function mount(surfaceId, surface, configuration) {
      const snapshot = artifactPolicySource.snapshotDataFields(
        configuration, REQUIRED_CONFIGURATION_FIELDS,
        OPTIONAL_CONFIGURATION_FIELDS
      );
      const domains = snapshot && typeof snapshot.domains !== "undefined"
        ? artifactPolicySource.snapshotArrayData(snapshot.domains)
        : undefined;
      if (disposed ||
          !validText(environment, surfaceId, MAX_SURFACE_ID_BYTES) ||
          !surface ||
          typeof surface.replaceChildren !== "function" ||
          !snapshot || domains === null ||
          !validText(environment, snapshot.session, MAX_SESSION_ID_BYTES) ||
          !artifactPolicySource.acceptsArtifactHTML(
            artifactPolicy,
            snapshot.artifactHTML
          ) ||
          (artifactPolicy.elevated &&
            typeof snapshot.materializedHTML !== "string") ||
          (typeof snapshot.materializedHTML !== "undefined" &&
            (!acceptMaterializedHTML ||
              !artifactPolicySource.acceptsMaterializedHTML(
                artifactPolicy,
                snapshot.materializedHTML,
                snapshot.materializedDigest
              ))) ||
          !primitives.isVerifiedArtifactBaseURL(snapshot.artifactBaseURL) ||
          !validDomains(environment, domains) ||
          (typeof snapshot.title !== "undefined" && !validText(
            environment, snapshot.title, MAX_TITLE_BYTES, true
          )) ||
          (typeof snapshot.onReady !== "undefined" &&
            typeof snapshot.onReady !== "function") ||
          (typeof snapshot.onError !== "undefined" &&
            typeof snapshot.onError !== "function") ||
          (!surfaces.has(surfaceId) && surfaces.size >= MAX_SURFACES)) {
        return false;
      }
      const admissionToken = artifactVerifierSource.beginVerifiedMount(
        artifactLifecycle, admission, artifactPolicy, snapshot);
      if (artifactPolicy.elevated && !admissionToken) return false;
      const frame = environment.document.createElement("iframe");
      if (surfaceId === "default") frame.id = "napplet-frame";
      frame.className = "napplet-frame";
      frame.setAttribute("sandbox", "allow-scripts");
      frame.setAttribute("referrerpolicy", "no-referrer");
      frame.setAttribute("aria-label", snapshot.title || "Napplet");
      frame.srcdoc = typeof snapshot.materializedHTML === "string"
        ? snapshot.materializedHTML
        : primitives.materialize(
          snapshot.artifactHTML, snapshot.artifactBaseURL, domains
        );
      if (artifactPolicy.elevated && !admission.activate(admissionToken)) {
        admission.settle(admissionToken);
        return false;
      }
      surface.replaceChildren(frame);
      const previous = surfaces.get(surfaceId);
      if (previous) {
        closeAcknowledgement(previous);
      }
      if (previous && typeof previous.frame.remove === "function") {
        previous.frame.remove();
      }
      const state = {
        frame,
        session: snapshot.session,
        onReady: snapshot.onReady,
        onError: snapshot.onError,
        acknowledgement: null,
        ready: false,
        loadCount: 0,
        messageWindowStartedAt: now(),
        messagesInWindow: 0,
        admissionToken,
        domains: Object.freeze(Array.from(new Set(
          ["shell"].concat(domains || [])
        )).sort())
      };
      if (typeof frame.addEventListener === "function") {
        frame.addEventListener("load", function observeNavigation() {
          if (surfaces.get(surfaceId) !== state) return;
          state.loadCount += 1;
          if (state.loadCount <= 1) return;
          unmount(surfaceId);
          try {
            if (state.onError) state.onError(surfaceId, "unexpected navigation");
          } catch (_) {}
        });
      }
      surfaces.set(surfaceId, state);
      admission.settle(admissionToken);
      return true;
    }
    function receive(surfaceId, envelope) {
      const state = surfaces.get(surfaceId);
      if (!state) {
        return false;
      }
      const projected = primitives.projectNativeEnvelope(
        envelope,
        state.domains.indexOf("resource") !== -1
      );
      if (projected === null) {
        return false;
      }
      if (projected.type === "shell.init" && !state.ready) {
        if (typeof environment.MessageChannel !== "function") {
          return false;
        }
        closeAcknowledgement(state);
        const channel = new environment.MessageChannel();
        state.acknowledgement = channel.port1;
        channel.port1.onmessage = function acknowledge(event) {
          if (surfaces.get(surfaceId) !== state) return;
          const accepted = event.data === "accepted";
          closeAcknowledgement(state);
          if (!accepted) {
            try {
              if (state.onError) state.onError(surfaceId, "shell.init rejected");
            } catch (_) {}
            return;
          }
          state.ready = true;
          try {
            if (state.onReady) state.onReady(surfaceId);
          } catch (_) {}
        };
        if (typeof channel.port1.start === "function") channel.port1.start();
        state.frame.contentWindow.postMessage(projected, "*", [channel.port2]);
      } else {
        state.frame.contentWindow.postMessage(projected, "*");
      }
      return true;
    }
    function unmount(surfaceId) {
      const state = surfaces.get(surfaceId);
      if (!state) {
        return false;
      }
      closeAcknowledgement(state);
      if (typeof state.frame.remove === "function") {
        state.frame.remove();
      }
      admission.release(state.admissionToken);
      surfaces.delete(surfaceId);
      return true;
    }
    const invalidateArtifactVerification = () => artifactPolicy.elevated &&
      artifactVerifierSource.invalidateArtifactLifecycle(artifactLifecycle);
    function verifyAndMaterialize(
      binding, artifactHTML, materialize, defaultDigestText, isCurrent
    ) {
      return artifactVerifierSource.verifyAndMaterialize(
        artifactLifecycle, environment, artifactPolicy, binding, artifactHTML,
        materialize, defaultDigestText, isCurrent
      );
    }
    function dispose() {
      if (disposed) return;
      disposed = true;
      for (const surfaceId of Array.from(surfaces.keys())) {
        unmount(surfaceId);
      }
      admission.dispose();
      artifactVerifierSource.invalidateArtifactLifecycle(artifactLifecycle, true);
      if (typeof environment.removeEventListener === "function")
        environment.removeEventListener("message", receiveNappletMessage);
    }
    if (typeof environment.addEventListener === "function") {
      environment.addEventListener("message", receiveNappletMessage);
    }
    return Object.freeze({ dispose, invalidateArtifactVerification, mount,
      receive, unmount, verifyAndMaterialize });
  }
  const exported = { MAX_SURFACES, MAX_ARTIFACT_HTML_BYTES,
    MAX_MATERIALIZED_HTML_BYTES, MAX_NAPPLET_MESSAGES_PER_SECOND,
    createSurfaceHost };
  if (global.document &&
      typeof global.addEventListener === "function" &&
      global.parent === global) {
    const host = createSurfaceHost(global);
    exported.mount = host.mount;
    exported.receive = host.receive;
    exported.unmount = host.unmount;
    global.__nmpTrustedShellMount = (configuration) => host.mount(
      "default", global.document.getElementById("surface"), configuration
    );
    global.__nmpTrustedShellReceive = (envelope) => host.receive("default", envelope);
  }
  global.NMPTrustedShellHost = Object.freeze(exported);
  if (typeof module !== "undefined" && module.exports)
    module.exports = Object.freeze(exported);
})(typeof window === "undefined" ? globalThis : window);
