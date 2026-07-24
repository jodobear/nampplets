# Runtime Workbench

Native macOS reference host for the Rust-owned NMP napplet runtime.

The app target contains only lifecycle wiring. Product code lives in
`RuntimeWorkbenchPackage`, the hardened WebKit host lives in
`platforms/apple`, and all artifact verification, principals, grants, session
domains, provider routing, and NMP ownership remain in Rust behind the
generated `Packages/NMPNativeRuntime` package.

The current reference flow loads the committed Good Morning evidence fixture:

1. Rust verifies its signed named-kind manifest and exact aggregate.
2. Rust installs the exact-build principal and derives signed requirements.
3. The session negotiates only implemented domains.
4. Swift mounts sealed verified bytes through the private `nmp-artifact`
   scheme.
5. The sandboxed iframe sends exact flat NAP envelopes through its mapped
   source window.

Good Morning receives the pinned identity, INC, outbox, resource, theme, and
link projections. A signed-out profile reaches the napplet's native signed-out
state; after native account activation, reads and approved writes stay behind
the Rust runtime and NMP facade. UI automation refuses the former degraded
screen and verifies the first mapped identity request.

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
