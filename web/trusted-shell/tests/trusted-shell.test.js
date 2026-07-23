"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const shell = require("../trusted-shell.js");
const policy = require("../trusted-shell-policy.js");

test("only the exact mapped iframe window may forward an envelope", () => {
  const mappedWindow = {};
  const spoofingWindow = {};
  const frame = { contentWindow: mappedWindow };
  const privileged = { type: "shell.ping", requestId: "one" };

  assert.deepEqual(
    shell.mappedEnvelope({ source: mappedWindow, data: privileged }, frame),
    privileged
  );
  assert.equal(
    shell.mappedEnvelope({ source: spoofingWindow, data: privileged }, frame),
    null
  );
  assert.equal(
    shell.mappedEnvelope({ source: null, data: privileged }, frame),
    null
  );
});

test("the iframe sandbox and CSP deny ambient origin, network, and storage power", () => {
  const html = fs.readFileSync(
    path.join(__dirname, "..", "trusted-shell.html"),
    "utf8"
  );
  const js = fs.readFileSync(
    path.join(__dirname, "..", "trusted-shell.js"),
    "utf8"
  );

  assert.match(js, /setAttribute\("sandbox", "allow-scripts"\)/);
  assert.doesNotMatch(js, /allow-same-origin/);
  assert.match(html, /connect-src 'none'/);
  assert.match(shell.sandboxPolicyContent(), /connect-src 'none'/);
  assert.match(shell.sandboxPolicyContent(), /default-src 'none'/);
  assert.match(shell.sandboxPolicyContent(), /worker-src 'none'/);
  assert.match(
    shell.sandboxPolicyContent(),
    /script-src 'unsafe-inline' nmp-artifact:/
  );
  assert.match(
    shell.sandboxPolicyContent(),
    /style-src 'unsafe-inline' nmp-artifact:/
  );
  assert.match(shell.sandboxPolicyContent(), /connect-src 'none'/);
  assert.match(shell.sandboxPolicyContent(), /base-uri nmp-artifact:/);
});

test("outer and inner CSP are generated from the single reviewed allowlist", () => {
  const html = fs.readFileSync(
    path.join(__dirname, "..", "trusted-shell.html"),
    "utf8"
  );
  const escapedPolicy = policy.outerPolicyContent()
    .replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

  assert.match(html, new RegExp(`content="${escapedPolicy}"`));
  assert.equal(policy.ALLOWLIST.artifactScheme, "nmp-artifact:");
  assert.match(policy.innerPolicyContent(), /connect-src 'none'/);
  assert.doesNotMatch(policy.innerPolicyContent(), /https?:/);
  assert.doesNotMatch(policy.innerPolicyContent(), /wss?:/);
});

test("oversized and non-JSON messages never cross the bridge", () => {
  const mappedWindow = {};
  const frame = { contentWindow: mappedWindow };

  assert.equal(
    shell.mappedEnvelope({ source: mappedWindow, data: "shell.ping" }, frame),
    null
  );
  assert.equal(
    shell.mappedEnvelope({
      source: mappedWindow,
      data: { payload: "x".repeat(shell.MAX_ENVELOPE_BYTES + 1) }
    }, frame),
    null
  );
});

test("materialization is parser-based rather than regex HTML rewriting", () => {
  const source = fs.readFileSync(
    path.join(__dirname, "..", "trusted-shell.js"),
    "utf8"
  );

  assert.match(source, /new global\.DOMParser\(\)/);
  assert.match(source, /parser\.parseFromString\(artifactHTML, "text\/html"\)/);
  assert.match(source, /head\.prepend\(policy\)/);
  assert.match(source, /head\.prepend\(base\)/);
  assert.doesNotMatch(source, /artifactHTML\.replace/);
  assert.match(
    shell.compatibilityPreludeSource(),
    /Object\.defineProperty\(window, "napplet"/
  );
  assert.equal(
    shell.isVerifiedArtifactBaseURL(
      "nmp-artifact://abcd1234-1234-4123-8123-abcdefabcdef/"
    ),
    true
  );
  assert.equal(
    shell.isVerifiedArtifactBaseURL("https://example.com/"),
    false
  );
});

test("the prelude performs the registry NAP-SHELL handshake exactly once", async () => {
  const listeners = new Map();
  const sent = [];
  const parent = {
    postMessage(envelope, target) {
      sent.push({ envelope: JSON.parse(JSON.stringify(envelope)), target });
    }
  };
  const context = {
    Map,
    Object,
    Promise,
    Set,
    Array,
    TypeError,
    Error,
    parent,
    queueMicrotask,
    addEventListener(type, listener) {
      listeners.set(type, listener);
    }
  };
  context.window = context;
  vm.runInNewContext(shell.compatibilityPreludeSource(["storage"]), context);

  assert.deepEqual(sent, [
    { envelope: { type: "shell.ready" }, target: "*" }
  ]);
  assert.equal(context.napplet.shell.supports("storage"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), []);

  let callbackCount = 0;
  context.napplet.shell.onReady(() => {
    callbackCount += 1;
  });
  listeners.get("message")({
    source: parent,
    data: {
      type: "shell.init",
      capabilities: { domains: ["shell", "storage", "storage"] },
      services: ["settings"]
    }
  });
  const environment = await context.napplet.shell.ready();
  await new Promise((resolve) => queueMicrotask(resolve));

  assert.deepEqual(
    JSON.parse(JSON.stringify(environment)),
    {
      capabilities: { domains: ["shell", "storage"] },
      services: ["settings"]
    }
  );
  assert.equal(context.napplet.shell.supports("storage"), true);
  assert.equal(context.napplet.shell.supports("unknown"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), ["settings"]);
  assert.equal(callbackCount, 1);

  listeners.get("message")({
    source: parent,
    data: {
      type: "shell.init",
      capabilities: { domains: ["shell", "theme"] },
      services: ["mutated"]
    }
  });
  assert.equal(context.napplet.shell.supports("theme"), false);
  assert.deepEqual(Array.from(context.napplet.shell.services), ["settings"]);
  assert.equal(sent.length, 1, "shell.init never causes another shell.ready");
});

test("prelude request envelopes use pinned flat fields and id correlation", async () => {
  const listeners = new Map();
  const sent = [];
  const parent = {
    postMessage(envelope, target) {
      sent.push({ envelope: JSON.parse(JSON.stringify(envelope)), target });
    }
  };
  const context = {
    Map,
    Object,
    Promise,
    Set,
    Array,
    TypeError,
    Error,
    parent,
    queueMicrotask,
    addEventListener(type, listener) {
      listeners.set(type, listener);
    }
  };
  context.window = context;
  vm.runInNewContext(shell.compatibilityPreludeSource(), context);

  const pending = context.napplet.shell.ping({
    source: "fixture",
    type: "forged.type",
    id: "forged-id"
  });
  assert.deepEqual(sent[1], {
    envelope: {
      type: "shell.ping",
      id: "request-1",
      source: "fixture"
    },
    target: "*"
  });
  listeners.get("message")({
    source: parent,
    data: {
      type: "shell.ping.result",
      id: "request-1",
      result: { ok: true }
    }
  });
  assert.deepEqual(
    JSON.parse(JSON.stringify(await pending)),
    { ok: true }
  );
});

test("storage projection matches the exact pinned async shim surface", async () => {
  const listeners = new Map();
  const sent = [];
  const parent = {
    postMessage(envelope) {
      sent.push(JSON.parse(JSON.stringify(envelope)));
    }
  };
  const context = {
    Map,
    Object,
    Promise,
    Set,
    Array,
    TypeError,
    Error,
    parent,
    queueMicrotask,
    addEventListener(type, listener) {
      listeners.set(type, listener);
    }
  };
  context.window = context;
  vm.runInNewContext(
    shell.compatibilityPreludeSource(["storage", "shell"]),
    context
  );

  assert.deepEqual(Object.keys(context.napplet).sort(), ["shell", "storage"]);
  assert.deepEqual(
    Object.keys(context.napplet.storage).sort(),
    ["getItem", "instance", "keys", "removeItem", "setItem"]
  );

  const shared = context.napplet.storage.getItem("theme");
  assert.deepEqual(sent[1], {
    type: "storage.get",
    id: "request-1",
    key: "theme"
  });
  listeners.get("message")({
    source: parent,
    data: {
      type: "storage.get.result",
      id: "request-1",
      value: "dark"
    }
  });
  assert.equal(await shared, "dark");

  const instance = context.napplet.storage.instance.setItem("draft", "hello");
  assert.deepEqual(sent[2], {
    type: "storage.set",
    id: "request-2",
    key: "draft",
    value: "hello",
    scope: "instance"
  });
  listeners.get("message")({
    source: parent,
    data: {
      type: "storage.set.result",
      id: "request-2"
    }
  });
  assert.equal(await instance, undefined);
});

test("prelude refuses domains it cannot faithfully project", () => {
  assert.throws(
    () => shell.compatibilityPreludeSource(["shell", "theme"]),
    /cannot project every negotiated domain/
  );
});

test("the Apple package snapshot exactly matches canonical trusted-shell bytes", () => {
  const canonicalRoot = path.join(__dirname, "..");
  const packagedRoot = path.join(
    __dirname,
    "..",
    "..",
    "..",
    "platforms",
    "apple",
    "Sources",
    "NMPNativeRuntimeApple",
    "Resources",
    "TrustedShell"
  );
  const relativeFiles = [
    "trusted-shell.html",
    "trusted-shell.css",
    "trusted-shell-policy.js",
    "trusted-shell.js",
    path.join("fixtures", "minimal-conformant-napplet.html"),
    path.join("fixtures", "external-assets", "index.html"),
    path.join("fixtures", "external-assets", "styles", "site.css"),
    path.join("fixtures", "external-assets", "scripts", "boot.js"),
    path.join("fixtures", "external-assets", "images", "verified.svg")
  ];

  for (const relativeFile of relativeFiles) {
    assert.equal(
      fs.readFileSync(path.join(packagedRoot, relativeFile), "utf8"),
      fs.readFileSync(path.join(canonicalRoot, relativeFile), "utf8"),
      `${relativeFile} must be refreshed with platforms/apple/scripts/sync-trusted-shell`
    );
  }
});
