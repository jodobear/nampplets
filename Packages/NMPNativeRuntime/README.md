# NMPNativeRuntime

Generated ergonomic Swift bindings for the Rust-owned native napplet runtime.
Application targets import `NMPNativeRuntime`; the C ABI module remains a
private target dependency.

Generate the package artifact from the repository root:

```sh
scripts/build-runtime-swift-xcframework.sh --arm64-only
cd Packages/NMPNativeRuntime
swift test
```

Use `--universal` for a distributable arm64/x86_64 macOS XCFramework.
