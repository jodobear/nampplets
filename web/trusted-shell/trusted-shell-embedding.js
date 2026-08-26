(function trustedShellEmbedding(global) {
  "use strict";
  const PROTOCOL_VERSION = 1;
  const MAX_PENDING_MOUNTS = 16;
  const MAX_PARENT_MESSAGES_PER_SECOND = 256;
  function moduleSource(value, path) {
    return value || (typeof require === "function" ? require(path) : null);
  }
  const primitiveSource = moduleSource(global.NMPTrustedShellPrimitives, "./trusted-shell.js");
  const hostSource = moduleSource(global.NMPTrustedShellHost, "./trusted-shell-surface-host.js");
  const contractSource = moduleSource(global.NMPTrustedShellEmbeddingContract, "./trusted-shell-embedding-contract.js");
  const transferSource = moduleSource(global.NMPTrustedShellEmbeddingTransfer, "./trusted-shell-embedding-transfer.js");
  const artifactPolicySource = moduleSource(
    global.NMPTrustedShellArtifactPolicy, "./trusted-shell-artifact-policy.js"
  );
  function createEmbeddingBridge(environment, dependencies = {}) {
    const primitives = dependencies.primitives || primitiveSource;
    const hostModule = dependencies.hostModule || hostSource;
    const digestText = dependencies.digestText || ((value) => artifactPolicySource.digestText(global, value));
    const now = dependencies.now || Date.now;
    if (!primitives || !hostModule || !contractSource || !transferSource ||
        !artifactPolicySource ||
        !environment || !environment.document ||
        !environment.parent || environment.parent === environment) {
      throw new Error("The trusted shell embedding bridge is unavailable");
    }
    const parentWindow = environment.parent;
    const artifactPolicy = artifactPolicySource
      .normalizeArtifactPolicy(dependencies.artifactPolicy);
    const contract = contractSource.createContract(primitives, hostModule, artifactPolicy);
    const admission = artifactPolicySource.createAdmission(artifactPolicy);
    const bindings = new Map();
    const pendingSurfaces = new Map();
    let pendingMounts = 0;
    let disposed = false;
    let messageWindowStartedAt = now();
    let messagesInWindow = 0;
    let parentRateLimited = false;
    let transfers = null;
    function post(message) {
      parentWindow.postMessage(Object.freeze(message), "*");
    }
    function currentBinding(surfaceId, session) {
      const state = bindings.get(surfaceId);
      return state && state.binding.session === session ? state : null;
    }
    const host = hostModule.createSurfaceHost(
      environment,
      primitives,
      {
        artifactPolicy,
        acceptMaterializedHTML: true,
        forwardEnvelope(message) {
          const state = currentBinding(message.surfaceId, message.session);
          if (!state) return;
          post({
            type: "nmp.outer.napplet",
            surfaceId: message.surfaceId,
            session: message.session,
            binding: state.binding,
            envelope: message.envelope
          });
        }
      }
    );
    function result(type, request, ok, error, binding = null) {
      post({
        type: `${type}.result`,
        requestId: request.requestId,
        surfaceId: request.surfaceId || null,
        session: request.session ||
          (request.configuration && request.configuration.session) || null,
        ok,
        error: error || null,
        binding
      });
    }
    function invalidate(surfaceId) {
      const state = bindings.get(surfaceId);
      if (state) admission.release(state.admissionToken);
      pendingSurfaces.delete(surfaceId);
      if (transfers) transfers.retire(surfaceId);
      bindings.delete(surfaceId);
      host.invalidateArtifactVerification();
      host.unmount(surfaceId);
    }
    async function mount(request, reservedAdmissionToken = null) {
      if (!contract.validMount(request)) {
        admission.settle(reservedAdmissionToken);
        return;
      }
      const domains = contract.snapshotDomains(request.configuration.domains);
      if (!domains) {
        admission.settle(reservedAdmissionToken);
        return;
      }
      if (pendingMounts >= MAX_PENDING_MOUNTS) {
        admission.settle(reservedAdmissionToken);
        result(request.type, request, false, "overloaded");
        return;
      }
      const admissionToken = reservedAdmissionToken || admission.begin();
      if (artifactPolicy.elevated && !admissionToken) {
        result(request.type, request, false, "overloaded");
        return;
      }
      const configuration = request.configuration;
      invalidate(request.surfaceId);
      const copied = Object.freeze({
        session: configuration.session,
        artifactHTML: configuration.artifactHTML,
        artifactBaseURL: configuration.artifactBaseURL,
        domains,
        title: configuration.title,
        binding: Object.freeze({ ...configuration.binding })
      });
      const mountToken = Object.freeze({ session: copied.session });
      pendingSurfaces.set(request.surfaceId, mountToken);
      pendingMounts += 1;
      try {
        const verified = await host.verifyAndMaterialize(
          copied.binding, copied.artifactHTML,
          () => primitives.materialize(
            copied.artifactHTML, copied.artifactBaseURL, copied.domains
          ), digestText,
          () => pendingSurfaces.get(request.surfaceId) === mountToken && !disposed
        );
        if (verified.status === "stale") return;
        if (verified.status !== "verified") {
          result(request.type, request, false, verified.status);
          return;
        }
        const materialized = verified.materializedHTML;
        const materializedDigest = verified.materializedDigest;
        const sealedBinding = Object.freeze({
          ...copied.binding,
          materializedDigest
        });
        const bindingState = { binding: sealedBinding, admissionToken };
        const mounted = host.mount(
          request.surfaceId,
          environment.document.getElementById("surface"),
          {
            session: copied.session,
            artifactHTML: copied.artifactHTML,
            binding: copied.binding,
            materializedHTML: materialized,
            materializedDigest,
            verificationReceipt: verified.verificationReceipt,
            artifactBaseURL: copied.artifactBaseURL,
            domains: copied.domains,
            title: copied.title,
            onReady() {
              if (bindings.get(request.surfaceId) !== bindingState) return;
              post({
                type: "nmp.outer.surface.ready",
                surfaceId: request.surfaceId,
                session: copied.session,
                binding: sealedBinding
              });
            },
            onError(_surfaceId, detail) {
              if (bindings.get(request.surfaceId) !== bindingState) return;
              invalidate(request.surfaceId);
              post({
                type: "nmp.outer.surface.error",
                surfaceId: request.surfaceId,
                session: copied.session,
                error: detail
              });
            }
          }
        );
        if (!mounted) {
          result(request.type, request, false, "mount-refused");
          return;
        }
        if (artifactPolicy.elevated && !admission.activate(admissionToken)) {
          host.unmount(request.surfaceId);
          return;
        }
        bindings.set(request.surfaceId, bindingState);
        pendingSurfaces.delete(request.surfaceId);
        result(request.type, request, true, null, sealedBinding);
      } catch (_) {
        if (pendingSurfaces.get(request.surfaceId) === mountToken) {
          result(request.type, request, false, "materialization-refused");
        }
      } finally {
        admission.settle(admissionToken);
        pendingMounts -= 1;
        if (pendingSurfaces.get(request.surfaceId) === mountToken) {
          pendingSurfaces.delete(request.surfaceId);
        }
      }
    }
    transfers = transferSource.createTransferManager(environment, {
      allocateBytes: dependencies.allocateTransferBytes,
      artifactPolicy,
      beforeBegin: invalidate,
      clearTimeout: dependencies.clearTransferTimeout,
      contract,
      onComplete(request, admissionToken) {
        void mount(request, admissionToken);
      },
      primitives,
      reserveAdmission: admission.begin,
      result,
      setTimeout: dependencies.setTransferTimeout,
      settleAdmission: admission.settle
    });
    function receiveParentMessage(event) {
      if (disposed || event.source !== parentWindow ||
          !primitives.isPlainObject(event.data)) return;
      const request = event.data;
      const bypassesParentRate = transfers.bypassesParentRate(request);
      const currentTime = now();
      if (currentTime - messageWindowStartedAt >= 1000) {
        messageWindowStartedAt = currentTime;
        messagesInWindow = 0;
        parentRateLimited = false;
      }
      if (!bypassesParentRate &&
          messagesInWindow >= MAX_PARENT_MESSAGES_PER_SECOND) {
        if (!parentRateLimited) {
          parentRateLimited = true;
          post({ type: "nmp.outer.rate-limited", scope: "parent" });
        }
        return;
      }
      if (!bypassesParentRate) messagesInWindow += 1;
      if (transfers.handle(request)) return;
      if (request.type === "nmp.outer.mount") {
        void mount(request);
        return;
      }
      if (request.type === "nmp.outer.deliver") {
      if (!contract.exactFields(request, [
        "envelope", "requestId", "session", "surfaceId", "type"
        ], primitives) || !contract.validRequestId(request.requestId) ||
          !contract.validSurfaceId(request.surfaceId) ||
          !contract.validSession(request.session)) return;
        const state = currentBinding(request.surfaceId, request.session);
        if (!state) {
          result(request.type, request, false, "stale");
        } else {
          const delivered = host.receive(request.surfaceId, request.envelope);
          result(
            request.type,
            request,
            delivered,
            delivered ? null : "deliver-refused"
          );
        }
      } else if (request.type === "nmp.outer.unmount") {
        if (!contract.exactFields(request, [
          "requestId", "session", "surfaceId", "type"
        ], primitives) || !contract.validRequestId(request.requestId) ||
            !contract.validSurfaceId(request.surfaceId) ||
            !contract.validSession(request.session)) return;
        const state = currentBinding(request.surfaceId, request.session);
        const pending = pendingSurfaces.get(request.surfaceId);
        const transferring = transfers.has(request.surfaceId, request.session);
        const removed = Boolean(state) || Boolean(
          pending && pending.session === request.session
        ) || transferring;
        if (removed) invalidate(request.surfaceId);
        result(request.type, request, removed, removed ? null : "stale");
      } else if (request.type === "nmp.outer.dispose") {
        if (!contract.exactFields(request, ["requestId", "type"], primitives) ||
            !contract.validRequestId(request.requestId)) return;
        post({
          type: "nmp.outer.dispose.result",
          requestId: request.requestId,
          ok: true
        });
        dispose();
      }
    }
    function dispose() {
      if (disposed) return;
      disposed = true;
      const surfaceIds = new Set([
        ...bindings.keys(),
        ...pendingSurfaces.keys()
      ]);
      for (const surfaceId of surfaceIds) invalidate(surfaceId);
      bindings.clear();
      pendingSurfaces.clear();
      transfers.dispose();
      admission.dispose();
      host.dispose();
      environment.removeEventListener("message", receiveParentMessage);
      environment.removeEventListener("pagehide", dispose);
    }
    environment.addEventListener("message", receiveParentMessage);
    environment.addEventListener("pagehide", dispose);
    post({ type: "nmp.outer.ready", version: PROTOCOL_VERSION });
    return Object.freeze({
      dispose,
      stateCounts() {
        return Object.freeze({
          bindings: bindings.size,
          pendingMounts,
          pendingSurfaces: pendingSurfaces.size,
          ...transfers.counts(),
          ...admission.counts()
        });
      }
    });
  }
  const exported = Object.freeze({ PROTOCOL_VERSION, MAX_PENDING_MOUNTS,
    MAX_PARENT_MESSAGES_PER_SECOND, createEmbeddingBridge });
  global.NMPTrustedShellEmbedding = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
