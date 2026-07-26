(function trustedShellPreludeDomainsModule(global) {
  "use strict";

  // The per-domain client sections of the compatibility prelude injected
  // into the sandboxed napplet frame. They live in their own reviewed
  // file only so every trusted-shell source stays under the repository's
  // 600-line ceiling (AGENTS.md). `trusted-shell.js` interpolates this
  // source verbatim, so the emitted prelude bytes are unchanged by the
  // split, and the shell's own tests assert the resulting behaviour.
  const DOMAIN_CLIENT_SOURCE = `  if (projectedDomains.indexOf("intent") !== -1) {
    function intentInvoke(intentRequest) {
      if (!isObject(intentRequest) || typeof intentRequest.archetype !== "string") {
        return Promise.reject(new TypeError("intent.invoke requires an archetype"));
      }
      return request(
        "intent.invoke",
        { request: intentRequest },
        function (message) {
          if (Object.prototype.hasOwnProperty.call(message, "result")) {
            return message.result;
          }
          throw new Error(errorMessage(message.error));
        },
        "intent.invoke.result",
        true,
        null,
        null,
        MAX_OUTBOX_REQUEST_TIMEOUT_MILLIS
      );
    }
    function intentOpen(archetype, payload, opts) {
      if (typeof archetype !== "string") {
        return Promise.reject(new TypeError("intent.open requires an archetype"));
      }
      var fields = { archetype: archetype, action: "open" };
      if (payload !== undefined) fields.payload = payload;
      if (isObject(opts)) {
        Object.keys(opts).forEach(function (key) {
          if (key !== "archetype" && key !== "action" && key !== "payload") {
            fields[key] = opts[key];
          }
        });
      }
      return intentInvoke(fields);
    }
    function intentAvailable(archetype) {
      if (typeof archetype !== "string") {
        return Promise.reject(new TypeError("intent.available requires an archetype"));
      }
      return request(
        "intent.available",
        { archetype: archetype },
        function (message) {
          if (!isObject(message.availability)) {
            throw new Error("intent.available.result missing availability");
          }
          return message.availability;
        },
        "intent.available.result",
        false,
        null,
        null,
        MAX_OUTBOX_REQUEST_TIMEOUT_MILLIS
      );
    }
    function intentHandlers() {
      return request(
        "intent.handlers",
        {},
        function (message) {
          return Array.isArray(message.handlers) ? message.handlers.slice() : [];
        },
        "intent.handlers.result",
        false,
        null,
        null,
        MAX_OUTBOX_REQUEST_TIMEOUT_MILLIS
      );
    }
    function intentOnChanged(handler) {
      if (typeof handler !== "function") {
        throw new TypeError("onChanged requires a function");
      }
      requireHandlerCapacity();
      intentChangedHandlers.add(handler);
      return closeHandle(function () {
        intentChangedHandlers.delete(handler);
      });
    }
    napplet.intent = Object.freeze({
      invoke: intentInvoke,
      open: intentOpen,
      available: intentAvailable,
      handlers: intentHandlers,
      onChanged: intentOnChanged
    });
  }
  if (projectedDomains.indexOf("inc") !== -1) {
    function incEmit(topic, payloadOrTags, legacyContent) {
      var payload;
      if (arguments.length >= 3) {
        if (legacyContent !== "") {
          try {
            payload = JSON.parse(legacyContent);
          } catch (_) {
            payload = legacyContent;
          }
        }
      } else if (arguments.length >= 2) {
        payload = payloadOrTags;
      }
      var fields = { topic: topic };
      if (payload !== undefined) fields.payload = payload;
      fireAndForget("inc.emit", fields);
    }
    function incOn(topic, handler) {
      if (typeof topic !== "string" || typeof handler !== "function") {
        throw new TypeError("inc.on requires a topic and function");
      }
      requireHandlerCapacity();
      var state = topicStates.get(topic);
      if (!state) {
        state = { handlers: new Set() };
        topicStates.set(topic, state);
        request("inc.subscribe", { topic: topic }).catch(function () {
          if (topicStates.get(topic) === state) {
            state.handlers.clear();
            topicStates.delete(topic);
          }
        });
      }
      state.handlers.add(handler);
      var active = true;
      return closeHandle(function () {
        if (!active) return;
        active = false;
        state.handlers.delete(handler);
        if (state.handlers.size === 0 && topicStates.get(topic) === state) {
          topicStates.delete(topic);
          fireAndForget("inc.unsubscribe", { topic: topic });
        }
      });
    }
    function createChannelHandle(channelId, peer) {
      var state = { closed: false, handlers: new Set() };
      channelStates.set(channelId, state);
      var handle = {
        id: channelId,
        peer: peer,
        emit: function (payload) {
          if (state.closed) throw new Error("INC channel is closed");
          var fields = { channelId: channelId };
          if (payload !== undefined) fields.payload = payload;
          fireAndForget("inc.channel.emit", fields);
        },
        on: function (handler) {
          if (typeof handler !== "function") {
            throw new TypeError("INC channel on requires a function");
          }
          if (state.closed) throw new Error("INC channel is closed");
          requireHandlerCapacity();
          state.handlers.add(handler);
          var active = true;
          return closeHandle(function () {
            if (!active) return;
            active = false;
            state.handlers.delete(handler);
          });
        },
        close: function () {
          if (state.closed) return;
          state.closed = true;
          state.handlers.clear();
          channelStates.delete(channelId);
          fireAndForget("inc.channel.close", { channelId: channelId });
        }
      };
      return Object.freeze(handle);
    }
    function openChannel(target) {
      if (channelStates.size + openingChannels >= MAX_CHANNELS) {
        return Promise.reject(new Error("INC channel capacity is full"));
      }
      openingChannels += 1;
      return request("inc.channel.open", { target: target }).then(function (message) {
        if (typeof message.channelId !== "string" ||
            typeof message.peer !== "string") {
          throw new Error("Runtime returned an invalid INC channel");
        }
        if (channelStates.has(message.channelId)) {
          fireAndForget("inc.channel.close", { channelId: message.channelId });
          throw new Error("Runtime returned a duplicate INC channel");
        }
        return createChannelHandle(message.channelId, message.peer);
      }).finally(function () {
        openingChannels -= 1;
      });
    }
    function listChannels() {
      return request("inc.channel.list", null, function (message) {
        if (!Array.isArray(message.channels)) {
          throw new Error("Runtime returned an invalid INC channel list");
        }
        return Object.freeze(message.channels.map(function (channel) {
          if (!isObject(channel) ||
              typeof channel.id !== "string" ||
              typeof channel.peer !== "string") {
            throw new Error("Runtime returned an invalid INC channel");
          }
          return Object.freeze({ id: channel.id, peer: channel.peer });
        }));
      });
    }
    function broadcastChannels(payload) {
      var fields = {};
      if (payload !== undefined) fields.payload = payload;
      fireAndForget("inc.channel.broadcast", fields);
    }
    napplet.inc = Object.freeze({
      emit: incEmit,
      on: incOn,
      channel: Object.freeze({
        open: openChannel,
        list: listChannels,
        broadcast: broadcastChannels
      })
    });
  }
  if (projectedDomains.indexOf("theme") !== -1) {
    napplet.theme = Object.freeze({
      get: function () {
        return request("theme.get", null, function (message) {
          return message.theme;
        });
      },
      onChanged: function (handler) {
        if (typeof handler !== "function") {
          throw new TypeError("theme.onChanged requires a function");
        }
        requireHandlerCapacity();
        themeChangedHandlers.add(handler);
        var active = true;
        return closeHandle(function () {
          if (!active) return;
          active = false;
          themeChangedHandlers.delete(handler);
        });
      }
    });
  }
  if (projectedDomains.indexOf("resource") !== -1) {
    function validResourceMime(value) {
      return typeof value === "string" &&
        value.length > 0 &&
        value.length <= MAX_RESOURCE_MIME_BYTES &&
        !/[\\u0000-\\u001f\\u007f]/.test(value);
    }
    function projectResourceInfo(message) {
      var info = message.info;
      var seenSchemes = new Set();
      if (!isObject(info) ||
          !Array.isArray(info.schemes) ||
          info.schemes.length > MAX_RESOURCE_INFO_SCHEMES ||
          !Number.isSafeInteger(info.maxBytes) ||
          info.maxBytes < 1 ||
          info.maxBytes > MAX_RESOURCE_INFO_LIMIT ||
          !Number.isSafeInteger(info.maxUrls) ||
          info.maxUrls < 1 ||
          info.maxUrls > MAX_RESOURCE_ITEMS) {
        throw new Error("Runtime returned invalid resource limits");
      }
      var schemes = info.schemes.map(function (item) {
        if (!isObject(item) ||
            typeof item.scheme !== "string" ||
            ["data", "https", "blossom"].indexOf(item.scheme) === -1 ||
            seenSchemes.has(item.scheme) ||
            typeof item.enabled !== "boolean") {
          throw new Error("Runtime returned an invalid resource scheme");
        }
        seenSchemes.add(item.scheme);
        return Object.freeze({
          scheme: item.scheme,
          enabled: item.enabled
        });
      });
      return Object.freeze({
        schemes: Object.freeze(schemes),
        maxBytes: info.maxBytes,
        maxUrls: info.maxUrls
      });
    }
    function projectResourceBlob(message) {
      if (!(message.blob instanceof Blob) ||
          !validResourceMime(message.mime) ||
          message.blob.type !== message.mime) {
        throw new Error("Runtime returned an invalid resource Blob");
      }
      return message.blob;
    }
    function projectResourceItems(message) {
      if (!Array.isArray(message.items) ||
          message.items.length > MAX_RESOURCE_ITEMS) {
        throw new Error("Runtime returned an invalid resource item list");
      }
      return Object.freeze(message.items.map(function (item) {
        if (!isObject(item) ||
            typeof item.url !== "string" ||
            item.url.length === 0 ||
            item.url.length > MAX_RESOURCE_URL_BYTES) {
          throw new Error("Runtime returned an invalid resource item");
        }
        if (item.ok === true) {
          var blob = projectResourceBlob(item);
          return Object.freeze({
            url: item.url,
            ok: true,
            blob: blob,
            mime: item.mime
          });
        }
        if (item.ok === false &&
            typeof item.error === "string" &&
            item.error.length > 0 &&
            item.error.length <= 128 &&
            typeof item.message === "string" &&
            item.message.length > 0 &&
            item.message.length <= 16 * 1024) {
          return Object.freeze({
            url: item.url,
            ok: false,
            error: item.error,
            message: item.message
          });
        }
        throw new Error("Runtime returned an invalid resource item");
      }));
    }
    function resourceRequest(action, fields, project) {
      if (environment === null ||
          environment.capabilities.domains.indexOf("resource") === -1) {
        return Promise.reject(
          new Error("NAP-RESOURCE is unavailable before shell.init")
        );
      }
      return request(
        "resource." + action,
        fields,
        project,
        "resource." + action + ".result",
        false,
        "resource." + action + ".error",
        action === "bytes" || action === "bytesMany"
          ? "resource.cancel"
          : null
      );
    }
    function boundedResourceURLs(urls) {
      if (urls === null || urls === undefined) return [];
      var values = [];
      var iteratorMethod = urls[Symbol.iterator];
      if (typeof iteratorMethod !== "function") {
        throw new TypeError("resource.bytesMany requires an iterable");
      }
      var iterator = iteratorMethod.call(urls);
      while (values.length <= MAX_RESOURCE_ITEMS) {
        var step = iterator.next();
        if (!isObject(step)) {
          throw new TypeError("resource.bytesMany iterator is invalid");
        }
        if (step.done) return values;
        values.push(String(step.value));
      }
      if (typeof iterator.return === "function") {
        try {
          iterator.return();
        } catch (_) {}
      }
      return values;
    }
    var resource = {
      info: function () {
        return resourceRequest("info", null, projectResourceInfo);
      },
      bytes: function (url) {
        return resourceRequest(
          "bytes",
          { url: String(url) },
          projectResourceBlob
        );
      },
      bytesMany: function (urls) {
        return resourceRequest(
          "bytesMany",
          { urls: boundedResourceURLs(urls) },
          projectResourceItems
        );
      },
      bytesAsObjectURL: function (url) {
        var handle = { url: "", revoke: function () {} };
        var objectUrl = "";
        var revoked = false;
        var ready = resource.bytes(url).then(function (blob) {
          if (revoked) return;
          if (resourceObjectUrls.size >= MAX_RESOURCE_OBJECT_URLS) {
            throw new RangeError("Resource object URL capacity is full");
          }
          if (typeof URL !== "function" &&
              (typeof URL !== "object" || URL === null)) {
            throw new Error("Resource object URLs are unavailable");
          }
          objectUrl = URL.createObjectURL(blob);
          resourceObjectUrls.add(objectUrl);
          handle.url = objectUrl;
          return objectUrl;
        });
        handle.revoke = function () {
          if (revoked) return;
          revoked = true;
          if (objectUrl) {
            if (resourceObjectUrls.delete(objectUrl)) {
              URL.revokeObjectURL(objectUrl);
            }
            objectUrl = "";
            handle.url = "";
          }
        };
        Object.defineProperty(handle, "ready", {
          value: ready,
          enumerable: false
        });
        return handle;
      }
    };
    napplet.resource = Object.freeze(resource);
  }
  if (projectedDomains.indexOf("link") !== -1) {
    function linkOptions(options) {
      if (options === undefined) return null;
      if (!isObject(options) ||
          Object.keys(options).some(function (key) {
            return key !== "label";
          }) ||
          (options.label !== undefined &&
           (typeof options.label !== "string" ||
            !utf8ByteLengthAtMost(options.label, MAX_LINK_LABEL_BYTES)))) {
        throw new TypeError(
          "link.open options may contain only a bounded string label"
        );
      }
      var projected = {};
      if (options.label !== undefined) projected.label = options.label;
      return projected;
    }
    function projectLinkOpen(message) {
      if (message.status === "opened" || message.status === "denied") {
        return Object.freeze({ status: message.status });
      }
      throw new Error(
        typeof message.error === "string" &&
        message.error.length > 0 &&
        message.error.length <= 4 * 1024
          ? message.error
          : "link open failed"
      );
    }
    napplet.link = Object.freeze({
      open: function (url, options) {
        if (environment === null ||
            environment.capabilities.domains.indexOf("link") === -1) {
          return Promise.reject(
            new Error("NAP-LINK is unavailable before shell.init")
          );
        }
        if (typeof url !== "string" ||
            url.length === 0 ||
            !utf8ByteLengthAtMost(url, MAX_LINK_URL_BYTES)) {
          return Promise.reject(
            new TypeError("link.open requires a bounded non-empty string URL")
          );
        }
        var fields = { url: url };
        var projectedOptions;
        try {
          projectedOptions = linkOptions(options);
        } catch (error) {
          return Promise.reject(error);
        }
        if (projectedOptions !== null) fields.options = projectedOptions;
        return request(
          "link.open",
          fields,
          projectLinkOpen,
          "link.open.result",
          true
        );
      }
    });
  }`;

  const preludeDomains = Object.freeze({
    DOMAIN_CLIENT_SOURCE
  });
  global.NMPTrustedShellPreludeDomains = preludeDomains;

  if (typeof module !== "undefined" && module.exports) {
    module.exports = preludeDomains;
  }
})(typeof window === "undefined" ? globalThis : window);
