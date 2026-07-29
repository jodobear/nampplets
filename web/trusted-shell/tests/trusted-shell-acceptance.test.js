"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const vm = require("node:vm");

const shell = require("../trusted-shell.js");

test("the prelude acknowledges the first exact NAP-SHELL environment", async () => {
  const listeners = new Map();
  const sent = [];
  class TestMessagePort {
    constructor() {
      this.messages = [];
      this.closed = false;
    }

    postMessage(message) { this.messages.push(message); }
    close() { this.closed = true; }
  }
  const parent = {
    postMessage(envelope, target) {
      sent.push({ envelope: JSON.parse(JSON.stringify(envelope)), target });
    }
  };
  const context = {
    Map, Object, Promise, Set, Array, Number, TypeError, RangeError, Error,
    MessagePort: TestMessagePort, parent, queueMicrotask, setTimeout, clearTimeout,
    addEventListener(type, listener) { listeners.set(type, listener); }
  };
  context.window = context;
  vm.runInNewContext(shell.compatibilityPreludeSource(["storage"]), context);

  assert.deepEqual(sent, [{ envelope: { type: "shell.ready" }, target: "*" }]);
  assert.equal(context.napplet.shell.supports("storage"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), []);

  let callbackCount = 0;
  context.napplet.shell.onReady(() => { callbackCount += 1; });
  const acceptedPort = new TestMessagePort();
  listeners.get("message")({
    source: parent,
    ports: [acceptedPort],
    data: {
      type: "shell.init",
      capabilities: { domains: ["shell", "storage", "storage"] },
      services: ["settings"]
    }
  });
  const environment = await context.napplet.shell.ready();
  await new Promise((resolve) => queueMicrotask(resolve));

  assert.deepEqual(JSON.parse(JSON.stringify(environment)), {
    capabilities: { domains: ["shell", "storage"] },
    services: ["settings"]
  });
  assert.equal(context.napplet.shell.supports("storage"), true);
  assert.equal(context.napplet.shell.supports("unknown"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), ["settings"]);
  assert.equal(callbackCount, 1);
  assert.deepEqual(acceptedPort.messages, ["accepted"]);
  assert.equal(acceptedPort.closed, true);

  const rejectedPort = new TestMessagePort();
  listeners.get("message")({
    source: parent,
    ports: [rejectedPort],
    data: {
      type: "shell.init",
      capabilities: { domains: ["shell", "theme"] },
      services: ["mutated"]
    }
  });
  assert.equal(context.napplet.shell.supports("theme"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), ["settings"]);
  assert.deepEqual(rejectedPort.messages, ["rejected"]);
  assert.equal(rejectedPort.closed, true);
  assert.equal(sent.length, 1, "shell.init never causes another shell.ready");
});
