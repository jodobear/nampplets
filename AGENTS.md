# NMP Native Runtime repository rules

These rules apply to the whole repository.

## Product boundary

- This repository is a native application runtime built above NMP.
- `/Users/pablofernandez/Work/nmp` is reference-only. Never edit, stage, reset,
  clean, stash, commit, or otherwise mutate that checkout from this repository.
- Consume only the supported NMP facade (`nmp::Engine`, the `NMP` Swift package,
  or the supported Kotlin wrapper). Mechanism crates and generated UniFFI types
  are not application APIs.
- Dependency direction is one way: platform/app -> runtime packages -> NMP
  facade. NMP never depends on this runtime.

## Ownership and architecture

- Rust owns product state machines, policy, validation, lifecycle, persistence,
  routing decisions, compatibility decisions, limits, and error semantics.
- Native code owns rendering, accessibility, platform lifecycle integration,
  and bounded execution of OS capabilities. It reports raw results to Rust.
- NMP is the only canonical Nostr event, replacement, deletion, routing, signer,
  pending-row, write-intent, and receipt owner.
- Runtime persistence may own installs, exact-build grants, component KV,
  workspaces, artifact indexes, and bounded activity facts. It must never become
  a second Nostr truth.
- The untrusted WebView iframe never receives the native bridge, `window.nostr`,
  key material, raw signer objects, unrestricted storage, or direct network.
- Every queue, stream, subscription, message, state frame, and resource class
  has a finite limit and observable refusal. Polling and sleep-check loops are
  prohibited.

## Workstream boundaries

- `conformance/`, `docs/`, and `compatibility.lock` own the pinned compatibility
  and security contract.
- `crates/artifact` owns verified artifact resolution and immutable bytes.
- `crates/runtime-core` owns principals, grants, sessions, quotas, and lifecycle.
- `crates/nap-bridge` owns NAP envelopes and provider dispatch.
- `crates/runtime-store` owns non-Nostr runtime persistence.
- `crates/surface` owns private surface descriptors, bindings, revisions, and
  typed actions.
- `crates/test-harness` owns deterministic service implementations; scenario
  contracts live under `conformance/test-services`.
- `apps/workbench-macos` owns the macOS reference shell and native presentation.

Do not redefine another workstream's public envelope, principal, persistence
schema, or lifecycle state machine. Coordinate changes at the owning boundary.

## Compatibility discipline

- `compatibility.lock` is authoritative. Upstream movement requires a dedicated
  compatibility change, regenerated fixtures and inventories, a new report, and
  explicit owner/security/NMP signoff.
- Existing accepted napplets must run without source or build changes.
- Unsupported domains are absent, not simulated by placeholder providers.
- Unknown well-formed message types are ignored at the compatibility boundary.
- Grants bind to `(manifest author, dTag, aggregateHash)`.
- Legacy compatibility becomes green before the private surface extension may
  be described as supported.

## Required gates

Run the narrow gate while iterating, then all applicable gates before handoff:

```sh
python3 -m unittest discover -s conformance/tests -p 'test_*.py'
python3 conformance/scripts/verify_baseline.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For Apple changes, build and test the shared `RuntimeWorkbench` scheme in the
macOS destination. For NMP-sensitive application work, run the architecture
scanner and document which D0-D10 rules the design discharges.
