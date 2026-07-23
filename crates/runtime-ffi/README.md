# Runtime FFI boundary

`nmp-native-runtime-ffi` is the production Rust-to-Swift boundary. UniFFI owns
the generated ABI mechanics; application targets import the generated
`NMPNativeRuntime` Swift module and never import the C module directly.

## Authority

- `VerifiedArtifact` is sealed by Rust after Nostr event id/signature,
  coordinate, path digest, aggregate, source, redirect, count, and byte-limit
  verification. Swift cannot construct one.
- Install, grant, launch, revoke, stop, crash, and mapped-envelope methods are
  fire-and-observe commands. Their semantic failures appear in bounded runtime
  snapshots/events; they do not throw FFI operation errors.
- The only throwing call is controller construction, where no runtime exists
  yet to own a semantic error state.
- Mapped envelopes accept only a Rust-issued active session id. Principal,
  profile, account, and grant claims in napplet bytes have no authority.
- Verified reads resolve an exact logical path against the sealed artifact for
  the active session. Native filesystem paths never cross the API.
- Artifact acquisition is a finite native capability callback. Rust supplies
  an explicit byte ceiling and approved candidate list, denies redirects, and
  rechecks response source, length, digest, and aggregate before committing.

## D0-D10 discharge

- D0/D4: `RuntimeApp` is the single product writer; this crate only validates,
  maps, and observes its commands and projections.
- D1/D5: observation emits the latest complete bounded app snapshot plus finite
  cursor-based event replay.
- D2/D3/D10: all Nostr acquisition, routing, persistence, and privacy stay
  behind the pinned `nmp::Engine` facade in `NmpDataPlane`.
- D6: command outcomes are state/events. Only pre-kernel open can throw.
- D7: the artifact callback returns raw HTTP facts and bytes; Rust decides
  trust, policy, and lifecycle.
- D8: observers are finitely admitted, updates conflate through watch channels,
  and stop/close are event-driven. There is no polling.
- D9: `RuntimeApp` receives one injected Rust clock; native callers never
  supply timestamps.

## Verification

```sh
cargo test -p nmp-native-runtime-ffi
cargo clippy -p nmp-native-runtime-ffi --all-targets -- -D warnings
scripts/tests/test-build-runtime-swift-xcframework.sh
scripts/build-runtime-swift-xcframework.sh --universal
xcodebuildmcp swift-package test \
  --package-path "$PWD/Packages/NMPNativeRuntime" \
  --output text
```

The Swift tests cross the actual ABI for open/snapshot/close, a callback-backed
signed artifact install/launch/read, and conflated observation teardown.
