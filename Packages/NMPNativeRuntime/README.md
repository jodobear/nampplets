# NMPNativeRuntime

Generated ergonomic Swift bindings for the Rust-owned native napplet runtime.
Application targets import `NMPNativeRuntime`; the C ABI module remains a
private target dependency.

Generate the package artifact from the repository root:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
scripts/build-runtime-swift-xcframework.sh --universal --check-bindings
swift test --package-path Packages/NMPNativeRuntime --parallel
```

The checked build requires generated UniFFI output to match the tracked Swift
source byte-for-byte and validates an exact arm64/x86_64 archive. Use
`--arm64-only` without `--check-bindings` only for local iteration. See the
[clean-checkout build](../../docs/build-from-clean-checkout.md) for the full
reproduction sequence.
