# NMP Native Runtime

An in-progress native Rust runtime and macOS reference host for NIP-5D
napplets. NMP remains the sole Nostr query, storage, routing, signing,
publication, receipt, and diagnostics engine; the runtime owns installation,
capabilities, WebView isolation, workspace bindings, and product policy.

The runnable macOS app now verifies a signed named-kind manifest in Rust,
installs its exact-build principal, derives its manifest requirements, launches
it through the Rust-owned session kernel, and serves only sealed artifact bytes
to a nested sandboxed WebView. The bundled Good Morning fixture currently runs
in its intentional limited-runtime state with the mandatory `shell` handshake
and pinned `storage` provider available.

This is not a claim that M2 or M3 is complete. The unchanged legacy evidence
still contains pinned-conformance failures and one native-only external-asset
lane, while the exact Kehto corpus cannot build from the currently available
offline package store. Good Morning also needs identity, INC, outbox, resource,
theme, and link providers for its full product behavior. The internal
`shell.ping` path remains only an M1 isolation-test canary.

The implementation follows the pinned contract in
[`nmp-native-runtime-spec-bundle`](nmp-native-runtime-spec-bundle/) in milestone
order:

1. lock and reproduce the compatibility baseline;
2. prove the nested WebView trust boundary;
3. run the unchanged legacy napplet corpus;
4. integrate native providers through NMP's public facade;
5. add host-owned surface bindings and typed actions;
6. complete and harden the macOS Runtime Workbench.

## Architecture boundary

```text
Runtime Workbench / platform host
        |
        +-- native runtime core (principals, grants, sessions, workspace)
        +-- trusted WebView shell -> sandboxed untrusted napplet iframe
        +-- NMP adapter -> supported `nmp` facade only
```

The WebView iframe never receives the native bridge, `window.nostr`, direct
network access, or caller-selected identity. Runtime persistence stores
installation and workspace facts, never a second authoritative Nostr cache.

## Repository

- `crates/` — Rust runtime, artifact, provider, surface, persistence, adapter,
  and deterministic test infrastructure.
- `Packages/NMPNativeRuntime/` — generated Swift bindings over the Rust
  composition root; its local XCFramework is reproducibly generated and
  intentionally ignored.
- `web/` — trusted shell and immutable compatibility fixtures.
- `apps/workbench-macos/` — native macOS reference app.
- `conformance/` — compatibility lock, fixture corpus, BDD scenarios, and
  reproducibility reports.
- `docs/` — threat model, provider matrix, compatibility record, and ADRs.

## Verification

```sh
cargo test --workspace
python3 conformance/scripts/verify_baseline.py
node --test web/trusted-shell/tests/trusted-shell.test.js
xcodebuildmcp swift-package test --package-path Packages/NMPNativeRuntime
xcodebuildmcp swift-package test --package-path platforms/apple
xcodebuildmcp swift-package test --package-path apps/workbench-macos/RuntimeWorkbenchPackage
```

Use the concrete Xcode project and scheme arguments documented by
`apps/workbench-macos/README.md`. The sibling `~/Work/nmp` checkout is a
read-only reference and must never be modified by this repository's workflows.

`compatibility.lock` remains intentionally `unratified` until real product,
compatibility, security, and NMP-boundary reviewers sign it. No blank signoff
is treated as approval.
