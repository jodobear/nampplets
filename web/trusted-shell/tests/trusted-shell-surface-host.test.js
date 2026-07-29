"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { MAX_SURFACES, createSurfaceHost } = require(
  "../trusted-shell-surface-host.js"
);

function createHarness() {
  const listeners = new Map();
  const forwarded = [];
  class TestPort {
    constructor() {
      this.closed = false;
      this.peer = null;
      this.onmessage = null;
    }

    postMessage(data) {
      if (!this.closed && !this.peer.closed && this.peer.onmessage) {
        this.peer.onmessage({ data });
      }
    }

    close() { this.closed = true; }
    start() {}
  }
  class TestMessageChannel {
    constructor() {
      this.port1 = new TestPort();
      this.port2 = new TestPort();
      this.port1.peer = this.port2;
      this.port2.peer = this.port1;
    }
  }
  const root = {
    payload: null,
    setAttribute(_name, value) { this.payload = value; },
    removeAttribute() { this.payload = null; }
  };
  const environment = {
    Event: class Event { constructor(type) { this.type = type; } },
    MessageChannel: TestMessageChannel,
    document: {
      documentElement: root,
      createElement() {
        return {
          attributes: {},
          contentWindow: {
            posted: [],
            postMessage(envelope, target, transfer = []) {
              this.posted.push({ envelope, target, transfer });
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
    target: "*",
    transfer: []
  }]);
});

test("surface readiness follows the prelude acknowledgement port", () => {
  const harness = createHarness();
  const target = surface();
  const ready = [];
  assert.equal(
    harness.host.mount(
      "acknowledged",
      target,
      { ...configuration("session-a"), onReady: (surfaceId) => ready.push(surfaceId) }
    ),
    true
  );

  assert.equal(harness.host.receive("acknowledged", {
    type: "shell.init",
    capabilities: { domains: ["shell"] },
    services: []
  }), true);
  const delivery = target.frame.contentWindow.posted[0];
  assert.equal(delivery.transfer.length, 1);
  assert.deepEqual(ready, []);
  delivery.transfer[0].postMessage("rejected");
  assert.deepEqual(ready, []);
  assert.equal(harness.host.receive("acknowledged", delivery.envelope), true);
  const accepted = target.frame.contentWindow.posted[1].transfer[0];
  accepted.postMessage("accepted");
  assert.deepEqual(ready, ["acknowledged"]);
  accepted.postMessage("accepted");
  assert.deepEqual(ready, ["acknowledged"]);
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

test("remounting a surface ID removes and unmaps its previous frame", () => {
  const harness = createHarness();
  const first = surface();
  const second = surface();
  assert.equal(harness.host.mount("stable", first, configuration("old")), true);
  const previousFrame = first.frame;

  assert.equal(harness.host.mount("stable", second, configuration("new")), true);
  assert.equal(previousFrame.removed, true);
  harness.listeners.get("message")({
    source: previousFrame.contentWindow,
    data: { type: "shell.ready" }
  });
  assert.equal(harness.forwarded.length, 0);
  harness.listeners.get("message")({
    source: second.frame.contentWindow,
    data: { type: "shell.ready" }
  });
  assert.equal(harness.forwarded[0].payload.session, "new");
});
