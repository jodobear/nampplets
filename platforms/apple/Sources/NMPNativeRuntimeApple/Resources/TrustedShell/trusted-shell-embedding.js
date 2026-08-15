(function trustedShellEmbedding(global) {
  "use strict";

  const PROTOCOL_VERSION = 1;
  const MAX_PENDING_MOUNTS = 16;
  const MAX_PARENT_MESSAGES_PER_SECOND = 256;
  const primitiveSource = global.NMPTrustedShellPrimitives ||
    (typeof require === "function" ? require("./trusted-shell.js") : null);
  const hostSource = global.NMPTrustedShellHost ||
    (typeof require === "function"
      ? require("./trusted-shell-surface-host.js")
      : null);
  const contractSource = global.NMPTrustedShellEmbeddingContract ||
    (typeof require === "function"
      ? require("./trusted-shell-embedding-contract.js")
      : null);

  async function defaultDigestText(value) {
    if (!global.crypto || !global.crypto.subtle) {
      throw new Error("SHA-256 is unavailable");
    }
    const bytes = new TextEncoder().encode(value);
    const digest = await global.crypto.subtle.digest("SHA-256", bytes);
    return Array.from(new Uint8Array(digest))
      .map((byte) => byte.toString(16).padStart(2, "0"))
      .join("");
  }

  function createEmbeddingBridge(environment, dependencies = {}) {
    const primitives = dependencies.primitives || primitiveSource;
    const hostModule = dependencies.hostModule || hostSource;
    const digestText = dependencies.digestText || defaultDigestText;
    const now = dependencies.now || Date.now;
    if (!primitives || !hostModule || !contractSource ||
        !environment || !environment.document ||
        !environment.parent || environment.parent === environment) {
      throw new Error("The trusted shell embedding bridge is unavailable");
    }
    const parentWindow = environment.parent;
    const contract = contractSource.createContract(primitives, hostModule);
    const bindings = new Map();
    const pendingSurfaces = new Map();
    let pendingMounts = 0;
    let disposed = false;
    let messageWindowStartedAt = now();
    let messagesInWindow = 0;
    let parentRateLimited = false;

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
      pendingSurfaces.delete(surfaceId);
      bindings.delete(surfaceId);
      host.unmount(surfaceId);
    }

    async function mount(request) {
      if (!contract.validMount(request)) return;
      if (pendingMounts >= MAX_PENDING_MOUNTS) {
        result(request.type, request, false, "overloaded");
        return;
      }
      const configuration = request.configuration;
      invalidate(request.surfaceId);
      const copied = Object.freeze({
        session: configuration.session,
        artifactHTML: configuration.artifactHTML,
        artifactBaseURL: configuration.artifactBaseURL,
        domains: Object.freeze(configuration.domains.slice()),
        title: configuration.title,
        binding: Object.freeze({ ...configuration.binding })
      });
      const mountToken = Object.freeze({ session: copied.session });
      pendingSurfaces.set(request.surfaceId, mountToken);
      let materialized;
      pendingMounts += 1;
      try {
        const artifactDigest = await digestText(copied.artifactHTML);
        if (pendingSurfaces.get(request.surfaceId) !== mountToken || disposed) return;
        if (artifactDigest !== copied.binding.artifactDigest) {
          result(request.type, request, false, "digest-mismatch");
          return;
        }
        materialized = primitives.materialize(
          copied.artifactHTML,
          copied.artifactBaseURL,
          copied.domains
        );
        const materializedDigest = await digestText(materialized);
        if (pendingSurfaces.get(request.surfaceId) !== mountToken || disposed) return;
        const sealedBinding = Object.freeze({
          ...copied.binding,
          materializedDigest
        });
        const bindingState = { binding: sealedBinding };
        const mounted = host.mount(
          request.surfaceId,
          environment.document.getElementById("surface"),
          {
            session: copied.session,
            artifactHTML: copied.artifactHTML,
            materializedHTML: materialized,
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
        bindings.set(request.surfaceId, bindingState);
        pendingSurfaces.delete(request.surfaceId);
        result(request.type, request, true, null, sealedBinding);
      } catch (_) {
        if (pendingSurfaces.get(request.surfaceId) === mountToken) {
          result(request.type, request, false, "materialization-refused");
        }
      } finally {
        pendingMounts -= 1;
        if (pendingSurfaces.get(request.surfaceId) === mountToken) {
          pendingSurfaces.delete(request.surfaceId);
        }
      }
    }

    function receiveParentMessage(event) {
      if (disposed || event.source !== parentWindow ||
          !primitives.isPlainObject(event.data)) return;
      const currentTime = now();
      if (currentTime - messageWindowStartedAt >= 1000) {
        messageWindowStartedAt = currentTime;
        messagesInWindow = 0;
        parentRateLimited = false;
      }
      if (messagesInWindow >= MAX_PARENT_MESSAGES_PER_SECOND) {
        if (!parentRateLimited) {
          parentRateLimited = true;
          post({ type: "nmp.outer.rate-limited", scope: "parent" });
        }
        return;
      }
      messagesInWindow += 1;
      const request = event.data;
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
        const removed = Boolean(state) || Boolean(
          pending && pending.session === request.session
        );
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
          pendingSurfaces: pendingSurfaces.size
        });
      }
    });
  }

  const exported = Object.freeze({
    PROTOCOL_VERSION,
    MAX_PENDING_MOUNTS,
    MAX_PARENT_MESSAGES_PER_SECOND,
    createEmbeddingBridge
  });
  if (global.document && global.parent && global.parent !== global) {
    createEmbeddingBridge(global);
  }
  global.NMPTrustedShellEmbedding = exported;
  if (typeof module !== "undefined" && module.exports) module.exports = exported;
})(typeof window === "undefined" ? globalThis : window);
