"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");

// The Apple package ships its own copy of the canonical trusted-shell
// resources. This file exists separately from trusted-shell.test.js only to
// keep both under the repository's 600-line ceiling; it is the sole owner of
// the byte-identity contract between the two copies.
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
    "trusted-shell-prelude-domains.js",
    "trusted-shell.js",
    "trusted-shell-surface-host.js",
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
