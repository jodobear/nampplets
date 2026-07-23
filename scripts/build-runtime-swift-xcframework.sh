#!/usr/bin/env bash
# Build generated Swift bindings and the macOS NMPNativeRuntime XCFramework.

set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/build-runtime-swift-xcframework.sh [OPTION]

Build the generated NMPNativeRuntime Swift bindings and macOS XCFramework.

Options:
  --arm64-only  build only the native Apple Silicon macOS slice
  --universal   build arm64 + x86_64 and combine them (default)
  -h, --help    show this help without building

CARGO_TARGET_DIR is honored. The deployment target defaults to macOS 13.0 and
may be overridden with MACOSX_DEPLOYMENT_TARGET.
USAGE
}

fail_usage() {
  echo "error: $1" >&2
  usage >&2
  exit 2
}

mode=universal
for argument in "$@"; do
  case "$argument" in
    --arm64-only)
      [[ "$mode" == universal || "$mode" == arm64 ]] \
        || fail_usage "conflicting architecture options"
      mode=arm64
      ;;
    --universal)
      [[ "$mode" == universal ]] || fail_usage "conflicting architecture options"
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      fail_usage "unknown option: $argument"
      ;;
  esac
done

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

crate=nmp-native-runtime-ffi
library=libnmp_native_runtime_ffi.a
arm_target=aarch64-apple-darwin
x86_target=x86_64-apple-darwin
deployment_target=${MACOSX_DEPLOYMENT_TARGET:-13.0}
target_value=${CARGO_TARGET_DIR:-target}
if [[ "$target_value" = /* ]]; then
  target_dir=$target_value
else
  target_dir=$repo_root/$target_value
fi
staging=$target_dir/runtime-swift-package
generated=$staging/generated
headers=$staging/headers
package_dir=$repo_root/Packages/NMPNativeRuntime
xcframework=$package_dir/NMPNativeRuntime.xcframework
swift_sources=$package_dir/Sources/NMPNativeRuntime

export LC_ALL=C
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-0}
common_cflags="-mmacosx-version-min=$deployment_target"

echo "== build arm64 macOS Rust library =="
MACOSX_DEPLOYMENT_TARGET=$deployment_target \
  CFLAGS="${CFLAGS:+$CFLAGS }$common_cflags" \
  CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$common_cflags" \
  cargo build -p "$crate" --release --target "$arm_target"
arm_library=$target_dir/$arm_target/release/$library

packaged_library=$arm_library
architectures=arm64
if [[ "$mode" == universal ]]; then
  echo "== build x86_64 macOS Rust library =="
  MACOSX_DEPLOYMENT_TARGET=$deployment_target \
    CFLAGS="${CFLAGS:+$CFLAGS }$common_cflags" \
    CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$common_cflags" \
    cargo build -p "$crate" --release --target "$x86_target"
  x86_library=$target_dir/$x86_target/release/$library
  mkdir -p "$staging"
  packaged_library=$staging/$library
  lipo -create "$arm_library" "$x86_library" -output "$packaged_library"
  architectures="arm64 x86_64"
fi

echo "== generate UniFFI Swift source and C module =="
rm -rf "$generated" "$headers"
mkdir -p "$generated" "$headers" "$swift_sources"
cargo run -p "$crate" --features bindgen --bin uniffi-bindgen -- generate \
  --library "$arm_library" \
  --language swift \
  --out-dir "$generated"
cp "$generated/nmp_native_runtime_ffiFFI.h" "$headers/"
cp "$generated/nmp_native_runtime_ffiFFI.modulemap" "$headers/module.modulemap"
cp "$generated/nmp_native_runtime_ffi.swift" \
  "$swift_sources/NMPNativeRuntime.swift"

echo "== create macOS XCFramework ($architectures) =="
rm -rf "$xcframework"
xcodebuild -create-xcframework \
  -library "$packaged_library" \
  -headers "$headers" \
  -output "$xcframework"

echo "== validate packaged architectures and generated binding =="
lipo -archs "$packaged_library"
test -s "$swift_sources/NMPNativeRuntime.swift"
test -s "$xcframework/Info.plist"

echo "XCFramework: $xcframework"
echo "Swift source: $swift_sources/NMPNativeRuntime.swift"
