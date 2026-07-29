(function trustedShellSurfaceHost(global) {
  "use strict";

  const MAX_SURFACES = 16;
  const MAX_SURFACE_ID_BYTES = 128;
  const primitiveSource = global.NMPTrustedShellPrimitives ||
    (typeof require === "function" ? require("./trusted-shell.js") : null);

  function validSurfaceId(value) {
    return typeof value === "string" &&
      value.length > 0 &&
      value.length <= MAX_SURFACE_ID_BYTES &&
      !/[\u0000-\u001f\u007f]/.test(value);
  }

  function createSurfaceHost(environment, suppliedPrimitives) {
    const primitives = suppliedPrimitives || primitiveSource;
    if (!primitives || !environment || !environment.document) {
      throw new Error("The trusted shell surface host is unavailable");
    }
    const surfaces = new Map();

    function forwardToNative(session, envelope) {
      const root = environment.document.documentElement;
      root.setAttribute("data-nmp-native-envelope", JSON.stringify({
        session,
        envelope
      }));
      environment.document.dispatchEvent(
        new environment.Event(primitives.bridgeEventName)
      );
      root.removeAttribute("data-nmp-native-envelope");
    }

    function receiveNappletMessage(event) {
      for (const state of surfaces.values()) {
        const envelope = primitives.mappedEnvelope(event, state.frame);
        if (envelope !== null) {
          forwardToNative(state.session, envelope);
          return;
        }
      }
    }

    function mount(surfaceId, surface, configuration) {
      if (!validSurfaceId(surfaceId) ||
          !surface ||
          typeof surface.replaceChildren !== "function" ||
          !primitives.isPlainObject(configuration) ||
          typeof configuration.session !== "string" ||
          typeof configuration.artifactHTML !== "string" ||
          !primitives.isVerifiedArtifactBaseURL(configuration.artifactBaseURL) ||
          (!Array.isArray(configuration.domains) &&
            typeof configuration.domains !== "undefined") ||
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
      frame.srcdoc = primitives.materialize(
        configuration.artifactHTML,
        configuration.artifactBaseURL,
        configuration.domains
      );
      surface.replaceChildren(frame);
      surfaces.set(surfaceId, {
        frame,
        session: configuration.session,
        domains: Object.freeze(Array.from(new Set(
          ["shell"].concat(configuration.domains || [])
        )).sort())
      });
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
      state.frame.contentWindow.postMessage(projected, "*");
      return true;
    }

    function unmount(surfaceId) {
      const state = surfaces.get(surfaceId);
      if (!state) {
        return false;
      }
      if (typeof state.frame.remove === "function") {
        state.frame.remove();
      }
      surfaces.delete(surfaceId);
      return true;
    }

    function dispose() {
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

  const exported = { MAX_SURFACES, createSurfaceHost };
  if (global.document && typeof global.addEventListener === "function") {
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
