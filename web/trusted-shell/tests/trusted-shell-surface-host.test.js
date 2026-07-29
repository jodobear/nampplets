"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { MAX_SURFACES, createSurfaceHost } = require(
  "../trusted-shell-surface-host.js"
);

function createHarness() {
  const listeners = new Map();
  const forwarded = [];
  const root = {
    payload: null,
    setAttribute(_name, value) { this.payload = value; },
    removeAttribute() { this.payload = null; }
  };
  const environment = {
    Event: class Event { constructor(type) { this.type = type; } },
    document: {
      documentElement: root,
      createElement() {
        return {
          attributes: {},
          contentWindow: {
            posted: [],
            postMessage(envelope, target) {
              this.posted.push({ envelope, target });
            }
          },
          setAttribute(name, value) { this.attributes[name] = value; },
          remove() { this.removed = true; }
        };
      },
      dispatchEvent(event) {
        forwarded.push({ event: event.type, payload: JSON.parse(root.payload) });
      }
    },
    addEventListener(type, listener) { listeners.set(type, listener); },
    removeEventListener(type) { listeners.delete(type); }
  };
  const primitives = {
    bridgeEventName: "nmp-native-envelope",
    isPlainObject(value) {
      return value !== null && typeof value === "object" && !Array.isArray(value);
    },
    isVerifiedArtifactBaseURL(value) { return value === "nmp-artifact://verified/"; },
    materialize(html) { return `materialized:${html}`; },
    mappedEnvelope(event, frame) {
      return event.source === frame.contentWindow ? event.data : null;
    },
    projectNativeEnvelope(envelope) { return envelope && envelope.type ? envelope : null; }
  };
  return {
    environment,
    forwarded,
    listeners,
    host: createSurfaceHost(environment, primitives)
  };
}

function surface() {
  return {
    frame: null,
    replaceChildren(frame) { this.frame = frame; }
  };
}

function configuration(session, domains = []) {
  return {
    session,
    artifactHTML: `<p>${session}</p>`,
    artifactBaseURL: "nmp-artifact://verified/",
    title: session,
    domains
  };
}

test("multiple surfaces retain independent source and native routing", () => {
  const harness = createHarness();
  const first = surface();
  const second = surface();
  assert.equal(harness.host.mount("first", first, configuration("session-a")), true);
  assert.equal(
    harness.host.mount("second", second, configuration("session-b", ["resource"])),
    true
  );

  harness.listeners.get("message")({
    source: first.frame.contentWindow,
    data: { type: "shell.ready" }
  });
  assert.deepEqual(harness.forwarded[0].payload, {
    session: "session-a",
    envelope: { type: "shell.ready" }
  });
  harness.listeners.get("message")({
    source: {},
    data: { type: "shell.ready", forged: true }
  });
  assert.equal(harness.forwarded.length, 1);

  assert.equal(harness.host.receive("second", { type: "identity.changed" }), true);
  assert.equal(first.frame.contentWindow.posted.length, 0);
  assert.deepEqual(second.frame.contentWindow.posted, [{
    envelope: { type: "identity.changed" },
    target: "*"
  }]);
});

test("surface count is bounded and unmount releases capacity", () => {
  const harness = createHarness();
  for (let index = 0; index < MAX_SURFACES; index += 1) {
    assert.equal(
      harness.host.mount(`surface-${index}`, surface(), configuration(`session-${index}`)),
      true
    );
  }
  assert.equal(
    harness.host.mount("overflow", surface(), configuration("overflow")),
    false
  );
  assert.equal(harness.host.unmount("surface-0"), true);
  assert.equal(
    harness.host.mount("replacement", surface(), configuration("replacement")),
    true
  );
  harness.host.dispose();
  assert.equal(harness.listeners.has("message"), false);
});
