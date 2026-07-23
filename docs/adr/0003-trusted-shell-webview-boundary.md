# ADR 0003: Keep the native bridge below a trusted WebView shell

- Status: Accepted architecture; baseline signoff pending
- Date: 2026-07-24
- Invariants: I-02, I-06, I-08
- Requirements: FR-C05 through FR-C09, FR-R02, FR-R06, FR-R07

## Context

Napplet code is untrusted but must execute unchanged. Exposing a generic native
bridge to its iframe would turn every provider validation mistake into native
authority. Origin comparison cannot identify an opaque-origin sandbox.

## Decision

Runtime policy and lifecycle execute in Rust. One coarse product surface uses
one platform WebView by default. The top-level WebView document is a tiny
trusted local shell. It creates exactly one inner napplet iframe with:

```text
sandbox="allow-scripts"
```

It omits `allow-same-origin`, loads verified bytes with `srcdoc` or private
verified materialization, and injects selected `window.napplet` domains before
all authored scripts. It never injects `window.nostr`.

The native script-message bridge exists only in the trusted shell. The napplet
uses `postMessage`. The shell binds the inner `Window` reference to an opaque
native-created session and forwards only JSON envelopes plus that session.
Native code derives principal and session from its own mapping and validates
again. Unknown windows are dropped; unknown well-formed types are ignored.
There is no generic `native.call(method, json)` surface.

Direct network, browser persistence, service workers, raw sockets, unverified
subresources, keys, and sibling access are denied. WebView teardown and crash
invalidate only session-owned work; automatic reload is finite.

## Consequences

- The trusted shell is security-sensitive and intentionally small.
- `event.origin` is not an authorization signal.
- All provider calls pay a typed native validation/policy boundary.
- Coarse WebViews avoid per-row process and lifecycle overhead.

## Verification

- A sibling/unmapped window cannot invoke a provider.
- The napplet cannot find the bridge or `window.nostr`.
- Fetch, WebSocket, storage, service-worker, and remote subresource attempts
  fail.
- Teardown closes session handles; crash does not stop NMP or workspace-owned
  bindings.
