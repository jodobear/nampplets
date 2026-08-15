"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const {
  MAX_ARTIFACT_HTML_BYTES,
  MAX_NAPPLET_MESSAGES_PER_SECOND,
  MAX_SURFACES,
  createSurfaceHost
} = require(
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
    TextEncoder,
    Event: class Event { constructor(type) { this.type = type; } },
    MessageChannel: TestMessageChannel,
    document: {
      documentElement: root,
      createElement() {
        const frameListeners = new Map();
        return {
          attributes: {},
          contentWindow: {
            posted: [],
            postMessage(envelope, target, transfer = []) {
              this.posted.push({ envelope, target, transfer });
            }
          },
          setAttribute(name, value) { this.attributes[name] = value; },
          addEventListener(type, listener) { frameListeners.set(type, listener); },
          emit(type) { frameListeners.get(type)(); },
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
  const failures = [];
  assert.equal(
    harness.host.mount(
      "acknowledged",
      target,
      {
        ...configuration("session-a"),
        onReady: (surfaceId) => ready.push(surfaceId),
        onError: (surfaceId, detail) => failures.push({ surfaceId, detail })
      }
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
  assert.deepEqual(failures, [{
    surfaceId: "acknowledged",
    detail: "shell.init rejected"
  }]);
  assert.equal(harness.host.receive("acknowledged", delivery.envelope), true);
  const accepted = target.frame.contentWindow.posted[1].transfer[0];
  accepted.postMessage("accepted");
  assert.deepEqual(ready, ["acknowledged"]);
  assert.equal(failures.length, 1);
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

test("public host refuses caller-supplied materialized HTML", () => {
  const harness = createHarness();
  assert.equal(harness.host.mount("sealed", surface(), {
    ...configuration("sealed"),
    materializedHTML: "<script>unreviewed()</script>"
  }), false);
});

test("disposing is terminal and refuses stale mounts", () => {
  const harness = createHarness();
  const target = surface();

  harness.host.dispose();
  harness.host.dispose();

  assert.equal(harness.host.mount("late", target, configuration("late")), false);
  assert.equal(target.frame, null);
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

test("artifact bytes, napplet message rate, and navigation are bounded", () => {
  const harness = createHarness();
  const target = surface();
  const oversized = configuration("oversized");
  oversized.artifactHTML = "x".repeat(MAX_ARTIFACT_HTML_BYTES + 1);
  assert.equal(harness.host.mount("oversized", target, oversized), false);

  const failures = [];
  assert.equal(harness.host.mount("bounded", target, {
    ...configuration("bounded"),
    onError: (_surfaceId, detail) => failures.push(detail)
  }), true);
  for (let index = 0; index < MAX_NAPPLET_MESSAGES_PER_SECOND + 1; index += 1) {
    harness.listeners.get("message")({
      source: target.frame.contentWindow,
      data: { type: "shell.ready", index }
    });
  }
  assert.equal(harness.forwarded.length, MAX_NAPPLET_MESSAGES_PER_SECOND);
  target.frame.emit("load");
  target.frame.emit("load");
  assert.equal(target.frame.removed, true);
  assert.deepEqual(failures, ["unexpected navigation"]);
});
