(function trustedShell(global) {
  "use strict";

  const MAX_ENVELOPE_BYTES = 64 * 1024;
  const bridgeEventName = "nmp-native-envelope";
  const policySource = global.NMPTrustedShellPolicy ||
    (typeof require === "function" ? require("./trusted-shell-policy.js") : null);
  let activeFrame = null;
  let nativeSessionToken = null;

  function isPlainObject(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }

  function mappedEnvelope(event, frame) {
    if (!frame || event.source !== frame.contentWindow) {
      return null;
    }
    if (!isPlainObject(event.data)) {
      return null;
    }
    let encoded;
    try {
      encoded = JSON.stringify(event.data);
    } catch (_) {
      return null;
    }
    if (encoded.length > MAX_ENVELOPE_BYTES) {
      return null;
    }
    return event.data;
  }

  function forwardToNative(envelope) {
    const payload = JSON.stringify({
      session: nativeSessionToken,
      envelope: envelope
    });
    const root = document.documentElement;
    root.setAttribute("data-nmp-native-envelope", payload);
    document.dispatchEvent(new Event(bridgeEventName));
    root.removeAttribute("data-nmp-native-envelope");
  }

  function compatibilityPreludeSource(domains) {
    const requested = domains === undefined ? ["shell"] : domains;
    if (!Array.isArray(requested) ||
        requested.some((domain) => domain !== "shell" && domain !== "storage")) {
      throw new Error("The trusted shell cannot project every negotiated domain");
    }
    const projectedDomains = Array.from(new Set(["shell"].concat(requested))).sort();
    return `(function () {
  "use strict";
  var projectedDomains = Object.freeze(${JSON.stringify(projectedDomains)});
  var nextRequest = 1;
  var pending = new Map();
  var environment = null;
  var readyHandlers = new Set();
  var resolveReady;
  var readyPromise = new Promise(function (resolve) {
    resolveReady = resolve;
  });
  function isObject(value) {
    return value !== null && typeof value === "object" && !Array.isArray(value);
  }
  function request(type, fields) {
    var id = "request-" + nextRequest++;
    var envelope = { type: type, id: id };
    if (isObject(fields)) {
      Object.keys(fields).forEach(function (key) {
        if (key !== "type" && key !== "id") envelope[key] = fields[key];
      });
    }
    return new Promise(function (resolve, reject) {
      pending.set(id, {
        resolve: resolve,
        reject: reject,
        resultType: type + ".result"
      });
      parent.postMessage(envelope, "*");
    });
  }
  function normalizeEnvironment(message) {
    if (!isObject(message.capabilities) || !Array.isArray(message.services)) return null;
    var domains = Array.isArray(message.capabilities.domains)
      ? message.capabilities.domains
      : [];
    if (!domains.every(function (domain) { return typeof domain === "string"; }) ||
        !message.services.every(function (service) { return typeof service === "string"; })) {
      return null;
    }
    return Object.freeze({
      capabilities: Object.freeze({
        domains: Object.freeze(Array.from(new Set(domains)))
      }),
      services: Object.freeze(message.services.slice())
    });
  }
  function acceptEnvironment(message) {
    if (environment !== null) return;
    var accepted = normalizeEnvironment(message);
    if (accepted === null) return;
    if (accepted.capabilities.domains.length !== projectedDomains.length ||
        accepted.capabilities.domains.some(function (domain, index) {
          return domain !== projectedDomains[index];
        })) {
      return;
    }
    environment = accepted;
    resolveReady(environment);
    readyHandlers.forEach(function (handler) {
      queueMicrotask(function () { handler(environment); });
    });
    readyHandlers.clear();
  }
  addEventListener("message", function (event) {
    if (event.source !== parent || !event.data || typeof event.data !== "object") return;
    if (event.data.type === "shell.init") {
      acceptEnvironment(event.data);
      return;
    }
    var operation = pending.get(event.data.id);
    if (!operation) return;
    if (event.data.type !== operation.resultType) return;
    pending.delete(event.data.id);
    if (event.data.error) {
      operation.reject(new Error(
        typeof event.data.error === "string"
          ? event.data.error
          : event.data.error.message || "Runtime request failed"
      ));
    } else {
      operation.resolve(
        Object.prototype.hasOwnProperty.call(event.data, "result")
          ? event.data.result
          : event.data
      );
    }
  });
  var shell = {};
  Object.defineProperties(shell, {
    supports: {
      enumerable: true,
      value: function (domain) {
        return typeof domain === "string" &&
          environment !== null &&
          environment.capabilities.domains.indexOf(domain) !== -1;
      }
    },
    services: {
      enumerable: true,
      get: function () {
        return environment === null ? Object.freeze([]) : environment.services;
      }
    },
    ready: {
      enumerable: true,
      value: function () { return readyPromise; }
    },
    onReady: {
      enumerable: true,
      value: function (handler) {
        if (typeof handler !== "function") throw new TypeError("onReady requires a function");
        var active = true;
        if (environment === null) {
          readyHandlers.add(handler);
        } else {
          queueMicrotask(function () { if (active) handler(environment); });
        }
        return Object.freeze({
          unsubscribe: function () {
            if (!active) return;
            active = false;
            readyHandlers.delete(handler);
          }
        });
      }
    },
    ping: {
      enumerable: true,
      value: function (fields) { return request("shell.ping", fields); }
    }
  });
  var napplet = { shell: Object.freeze(shell) };
  if (projectedDomains.indexOf("storage") !== -1) {
    function storageGet(key, scope) {
      var fields = { key: key };
      if (scope === "instance") fields.scope = scope;
      return request("storage.get", fields).then(function (message) {
        return message.value;
      });
    }
    function storageSet(key, value, scope) {
      var fields = { key: key, value: value };
      if (scope === "instance") fields.scope = scope;
      return request("storage.set", fields).then(function () {});
    }
    function storageRemove(key, scope) {
      var fields = { key: key };
      if (scope === "instance") fields.scope = scope;
      return request("storage.remove", fields).then(function () {});
    }
    function storageKeys(scope) {
      var fields = {};
      if (scope === "instance") fields.scope = scope;
      return request("storage.keys", fields).then(function (message) {
        return message.keys;
      });
    }
    var instanceStorage = Object.freeze({
      getItem: function (key) { return storageGet(key, "instance"); },
      setItem: function (key, value) { return storageSet(key, value, "instance"); },
      removeItem: function (key) { return storageRemove(key, "instance"); },
      keys: function () { return storageKeys("instance"); }
    });
    napplet.storage = Object.freeze({
      getItem: function (key) { return storageGet(key); },
      setItem: function (key, value) { return storageSet(key, value); },
      removeItem: function (key) { return storageRemove(key); },
      keys: function () { return storageKeys(); },
      instance: instanceStorage
    });
  }
  Object.defineProperty(window, "napplet", {
    configurable: false,
    enumerable: true,
    writable: false,
    value: Object.freeze(napplet)
  });
  parent.postMessage({ type: "shell.ready" }, "*");
})();`;
  }

  function sandboxPolicyContent() {
    if (!policySource) {
      throw new Error("The trusted shell policy is unavailable");
    }
    return policySource.innerPolicyContent();
  }

  function isVerifiedArtifactBaseURL(value) {
    return typeof value === "string" &&
      /^nmp-artifact:\/\/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\/$/.test(value);
  }

  function materialize(artifactHTML, artifactBaseURL, domains) {
    if (typeof global.DOMParser !== "function") {
      throw new Error("The trusted shell requires an HTML parser");
    }
    if (!isVerifiedArtifactBaseURL(artifactBaseURL)) {
      throw new Error("The verified artifact base URL is invalid");
    }

    // Parsing into an inert document is security-critical. String/regex
    // rewriting cannot model the HTML parser's error recovery, and can place
    // the bootstrap after an executable node in malformed-but-valid input
    // such as `<script>…</script><head>`.
    const parser = new global.DOMParser();
    const parsed = parser.parseFromString(artifactHTML, "text/html");
    const head = parsed.head;
    if (!head) {
      throw new Error("The artifact did not produce an HTML head");
    }

    const policy = parsed.createElement("meta");
    policy.setAttribute("http-equiv", "Content-Security-Policy");
    policy.setAttribute("content", sandboxPolicyContent());

    const base = parsed.createElement("base");
    base.setAttribute("href", artifactBaseURL);

    const prelude = parsed.createElement("script");
    prelude.textContent = compatibilityPreludeSource(domains);

    // The enforced policy is the first child and the compatibility bootstrap
    // is the second. DOMParser is inert, so no authored executable node can
    // run before these nodes are serialized into the sandboxed srcdoc.
    head.prepend(prelude);
    head.prepend(base);
    head.prepend(policy);

    return "<!doctype html>\n" + parsed.documentElement.outerHTML;
  }

  global.__nmpTrustedShellMount = function mount(configuration) {
    if (!isPlainObject(configuration) ||
        typeof configuration.session !== "string" ||
        typeof configuration.artifactHTML !== "string" ||
        !isVerifiedArtifactBaseURL(configuration.artifactBaseURL)) {
      return false;
    }
    nativeSessionToken = configuration.session;
    const frame = document.createElement("iframe");
    frame.id = "napplet-frame";
    frame.setAttribute("sandbox", "allow-scripts");
    frame.setAttribute("referrerpolicy", "no-referrer");
    frame.setAttribute("aria-label", configuration.title || "Napplet");
    frame.srcdoc = materialize(
      configuration.artifactHTML,
      configuration.artifactBaseURL,
      configuration.domains
    );
    const surface = document.getElementById("surface");
    surface.replaceChildren(frame);
    activeFrame = frame;
    return true;
  };

  global.__nmpTrustedShellReceive = function receive(envelope) {
    if (!activeFrame || !isPlainObject(envelope)) {
      return false;
    }
    activeFrame.contentWindow.postMessage(envelope, "*");
    return true;
  };

  if (typeof global.addEventListener === "function") {
    global.addEventListener("message", function receiveNappletMessage(event) {
      const envelope = mappedEnvelope(event, activeFrame);
      if (envelope !== null) {
        forwardToNative(envelope);
      }
    });
  }

  if (typeof module !== "undefined" && module.exports) {
    module.exports = {
      MAX_ENVELOPE_BYTES,
      mappedEnvelope,
      materialize,
      sandboxPolicyContent,
      isVerifiedArtifactBaseURL,
      compatibilityPreludeSource
    };
  }
})(typeof window === "undefined" ? globalThis : window);
