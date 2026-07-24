# Runtime Workbench

Native macOS reference host for the Rust-owned NMP napplet runtime.

The app target contains only lifecycle wiring. Product code lives in
`RuntimeWorkbenchPackage`, the hardened WebKit host lives in
`platforms/apple`, and all artifact verification, principals, grants, session
domains, provider routing, and NMP ownership remain in Rust behind the
generated `Packages/NMPNativeRuntime` package.

The current reference flow loads the committed Good Morning evidence fixture:

1. Rust verifies its signed named-kind manifest and exact aggregate.
2. Rust installs the exact-build principal and derives its pinned compatibility
   request profile.
3. Swift presents the Rust-owned exact-build permission review and applies one
   complete atomic decision batch.
4. A separate launch command negotiates only permitted, implemented domains.
5. Swift mounts sealed verified bytes through the private `nmp-artifact`
   scheme.
6. The sandboxed iframe sends exact flat NAP envelopes through its mapped
   source window.

Good Morning can launch with identity, INC, outbox, and theme after explicit
review. Resource and link remain honestly unavailable until qualified bounded
native executors are registered, so those optional features degrade without
being advertised. UI automation covers the permission-to-launch path; the
legacy-host corpus retains the deliberate missing-capability evidence.

## Open in Xcode

Open `RuntimeWorkbench.xcworkspace`, not the project directly. The shared
`RuntimeWorkbench` scheme runs:

- Workbench package tests;
- Apple runtime package tests used by the app;
- the signed Good Morning UI flow.

## Command-line verification

From the repository root:

```sh
scripts/build-runtime-swift-xcframework.sh --universal --check-bindings
xcodebuildmcp swift-package test \
  --package-path apps/workbench-macos/RuntimeWorkbenchPackage
xcodebuildmcp macos test \
  --workspace-path apps/workbench-macos/RuntimeWorkbench.xcworkspace \
  --scheme RuntimeWorkbench \
  --derived-data-path /tmp/nampplets-runtime-workbench-derived-data
```

The development app is sandboxed and uses Application Support for its runtime,
NMP, and verified-artifact stores. Untrusted napplet content receives no native
bridge, ambient network, persistent WebKit storage, `window.nostr`, or caller-
selected identity.
