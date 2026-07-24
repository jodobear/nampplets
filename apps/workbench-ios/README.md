# Runtime Workbench (iOS)

Native iOS reference host for the Rust-owned NMP napplet runtime, sharing
`RuntimeWorkbenchFeature` with the macOS reference host in
`apps/workbench-macos`.

The app target contains only lifecycle wiring. Product code lives in
`apps/workbench-macos/RuntimeWorkbenchPackage`, the hardened WebKit host lives
in `platforms/apple`, and all artifact verification, principals, grants,
session domains, provider routing, and NMP ownership remain in Rust behind the
generated `Packages/NMPNativeRuntime` package.

## Open in Xcode

Open `RuntimeWorkbenchiOS.xcworkspace`, not the project directly. The shared
`RuntimeWorkbenchiOS` scheme builds and runs the same feature package the
macOS app uses, adapted to a compact iOS toolbar and sheet layout.

## Command-line verification

From the repository root:

```sh
scripts/build-runtime-swift-xcframework.sh --arm64-only
xcodebuildmcp simulator build-and-run \
  --workspace-path apps/workbench-ios/RuntimeWorkbenchiOS.xcworkspace \
  --scheme RuntimeWorkbenchiOS \
  --simulator-name 'iPhone 17'
```

The app is sandboxed and uses Application Support for its runtime, NMP, and
verified-artifact stores. Untrusted napplet content receives no native bridge,
ambient network, persistent WebKit storage, `window.nostr`, or caller-selected
identity.
