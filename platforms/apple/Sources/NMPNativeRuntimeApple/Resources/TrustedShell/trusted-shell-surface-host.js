(function trustedShellSurfaceHost(global) {
  "use strict";

  const MAX_SURFACES = 16;
  const MAX_SURFACE_ID_BYTES = 128;
  const MAX_SESSION_ID_BYTES = 256;
  const MAX_ARTIFACT_HTML_BYTES = 8 * 1024 * 1024;
  const MAX_MATERIALIZED_HTML_BYTES = 16 * 1024 * 1024;
  const MAX_TITLE_BYTES = 1024;
  const MAX_DOMAINS = 64;
  const MAX_DOMAIN_BYTES = 64;
  const MAX_NAPPLET_MESSAGES_PER_SECOND = 256;
  const primitiveSource = global.NMPTrustedShellPrimitives ||
    (typeof require === "function" ? require("./trusted-shell.js") : null);

  function byteLength(environment, value) {
    if (typeof environment.TextEncoder === "function") {
      return new environment.TextEncoder().encode(value).byteLength;
    }
    return value.length * 3;
  }

  function validText(environment, value, maximumBytes, allowEmpty = false) {
    return typeof value === "string" &&
      (allowEmpty || value.length > 0) &&
      byteLength(environment, value) <= maximumBytes &&
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
    if (!primitives || !environment || !environment.document) {
      throw new Error("The trusted shell surface host is unavailable");
    }
    const forwardEnvelope = typeof options.forwardEnvelope === "function"
      ? options.forwardEnvelope
      : null;
    const acceptMaterializedHTML = options.acceptMaterializedHTML === true;
    const now = typeof options.now === "function" ? options.now : Date.now;
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
      if (disposed ||
          !validText(environment, surfaceId, MAX_SURFACE_ID_BYTES) ||
          !surface ||
          typeof surface.replaceChildren !== "function" ||
          !primitives.isPlainObject(configuration) ||
          !validText(environment, configuration.session, MAX_SESSION_ID_BYTES) ||
          typeof configuration.artifactHTML !== "string" ||
          byteLength(environment, configuration.artifactHTML) >
            MAX_ARTIFACT_HTML_BYTES ||
          (typeof configuration.materializedHTML !== "undefined" &&
            (!acceptMaterializedHTML ||
              typeof configuration.materializedHTML !== "string" ||
              byteLength(environment, configuration.materializedHTML) >
                MAX_MATERIALIZED_HTML_BYTES)) ||
          !primitives.isVerifiedArtifactBaseURL(configuration.artifactBaseURL) ||
          !validDomains(environment, configuration.domains) ||
          (typeof configuration.title !== "undefined" &&
            !validText(
              environment,
              configuration.title,
              MAX_TITLE_BYTES,
              true
            )) ||
          (typeof configuration.onReady !== "undefined" &&
            typeof configuration.onReady !== "function") ||
          (typeof configuration.onError !== "undefined" &&
            typeof configuration.onError !== "function") ||
          (!surfaces.has(surfaceId) && surfaces.size >= MAX_SURFACES)) {
        return false;
      }
      const frame = environment.document.createElement("iframe");
      if (surfaceId === "default") {
        frame.id = "napplet-frame";
      }
      frame.className = "napplet-frame";
      frame.setAttribute("sandbox", "allow-scripts");
      frame.setAttribute("referrerpolicy", "no-referrer");
      frame.setAttribute("aria-label", configuration.title || "Napplet");
      frame.srcdoc = typeof configuration.materializedHTML === "string"
        ? configuration.materializedHTML
        : primitives.materialize(
          configuration.artifactHTML,
          configuration.artifactBaseURL,
          configuration.domains
        );
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
        session: configuration.session,
        onReady: configuration.onReady,
        onError: configuration.onError,
        acknowledgement: null,
        ready: false,
        loadCount: 0,
        messageWindowStartedAt: now(),
        messagesInWindow: 0,
        domains: Object.freeze(Array.from(new Set(
          ["shell"].concat(configuration.domains || [])
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
      surfaces.delete(surfaceId);
      return true;
    }

    function dispose() {
      if (disposed) return;
      disposed = true;
      for (const surfaceId of Array.from(surfaces.keys())) {
        unmount(surfaceId);
      }
      if (typeof environment.removeEventListener === "function") {
        environment.removeEventListener("message", receiveNappletMessage);
      }
    }

    if (typeof environment.addEventListener === "function") {
      environment.addEventListener("message", receiveNappletMessage);
    }
    return Object.freeze({ mount, receive, unmount, dispose });
  }

  const exported = {
    MAX_SURFACES,
    MAX_ARTIFACT_HTML_BYTES,
    MAX_MATERIALIZED_HTML_BYTES,
    MAX_NAPPLET_MESSAGES_PER_SECOND,
    createSurfaceHost
  };
  if (global.document &&
      typeof global.addEventListener === "function" &&
      global.parent === global) {
    const host = createSurfaceHost(global);
    exported.mount = host.mount;
    exported.receive = host.receive;
    exported.unmount = host.unmount;
    global.__nmpTrustedShellMount = function mountDefault(configuration) {
      return host.mount(
        "default",
        global.document.getElementById("surface"),
        configuration
      );
    };
    global.__nmpTrustedShellReceive = function receiveDefault(envelope) {
      return host.receive("default", envelope);
    };
  }
  global.NMPTrustedShellHost = Object.freeze(exported);

  if (typeof module !== "undefined" && module.exports) {
    module.exports = Object.freeze(exported);
  }
})(typeof window === "undefined" ? globalThis : window);
