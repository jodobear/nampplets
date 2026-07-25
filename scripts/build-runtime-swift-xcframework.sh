#!/usr/bin/env bash
# Build generated Swift bindings and the multi-platform NMPNativeRuntime XCFramework.

set -euo pipefail

usage() {
  cat <<'USAGE'
Usage: scripts/build-runtime-swift-xcframework.sh [OPTION]

Build the generated NMPNativeRuntime Swift bindings and the multi-platform
NMPNativeRuntime XCFramework (macOS, iOS device, iOS Simulator).

Options:
  --arm64-only  build only the native Apple Silicon macOS slice
  --universal   build arm64 + x86_64 macOS and combine them (default)
  --no-ios      skip the iOS device and iOS Simulator slices
  --check-bindings
                 refuse to replace a stale checked-in Swift binding
  -h, --help    show this help without building

CARGO_TARGET_DIR is honored. The macOS deployment target defaults to 13.0 and
may be overridden with MACOSX_DEPLOYMENT_TARGET; the iOS deployment target
defaults to 17.0 and may be overridden with IPHONEOS_DEPLOYMENT_TARGET. CI
uses --check-bindings so the generated Swift API must byte-match the
checked-in package source.
USAGE
}

fail_usage() {
  echo "error: $1" >&2
  usage >&2
  exit 2
}

mode=universal
check_bindings=false
ios_enabled=true
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
    --no-ios)
      ios_enabled=false
      ;;
    --check-bindings)
      check_bindings=true
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

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$repo_root"

crate=nmp-native-runtime-ffi
library=libnmp_native_runtime_ffi.a
arm_target=aarch64-apple-darwin
x86_target=x86_64-apple-darwin
ios_device_target=aarch64-apple-ios
ios_sim_arm_target=aarch64-apple-ios-sim
ios_sim_x86_target=x86_64-apple-ios
deployment_target=${MACOSX_DEPLOYMENT_TARGET:-13.0}
ios_deployment_target=${IPHONEOS_DEPLOYMENT_TARGET:-17.0}
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
packaged_swift=$swift_sources/NMPNativeRuntime.swift

export LC_ALL=C
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-0}
common_cflags="-mmacosx-version-min=$deployment_target"

echo "== build arm64 macOS Rust library =="
MACOSX_DEPLOYMENT_TARGET=$deployment_target \
  CFLAGS="${CFLAGS:+$CFLAGS }$common_cflags" \
  CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$common_cflags" \
  cargo build -p "$crate" --locked --release --target "$arm_target"
arm_library=$target_dir/$arm_target/release/$library

packaged_library=$arm_library
architectures=arm64
if [[ "$mode" == universal ]]; then
  echo "== build x86_64 macOS Rust library =="
  MACOSX_DEPLOYMENT_TARGET=$deployment_target \
    CFLAGS="${CFLAGS:+$CFLAGS }$common_cflags" \
    CXXFLAGS="${CXXFLAGS:+$CXXFLAGS }$common_cflags" \
    cargo build -p "$crate" --locked --release --target "$x86_target"
  x86_library=$target_dir/$x86_target/release/$library
  mkdir -p "$staging"
  packaged_library=$staging/$library
  lipo -create "$arm_library" "$x86_library" -output "$packaged_library"
  architectures="arm64 x86_64"
fi

ios_device_library=
ios_sim_library=
if [[ "$ios_enabled" == true ]]; then
  echo "== build iOS device Rust library =="
  IPHONEOS_DEPLOYMENT_TARGET=$ios_deployment_target \
    cargo build -p "$crate" --locked --release --target "$ios_device_target"
  ios_device_library=$target_dir/$ios_device_target/release/$library

  echo "== build arm64 iOS Simulator Rust library =="
  IPHONEOS_DEPLOYMENT_TARGET=$ios_deployment_target \
    cargo build -p "$crate" --locked --release --target "$ios_sim_arm_target"
  ios_sim_arm_library=$target_dir/$ios_sim_arm_target/release/$library

  ios_sim_library=$ios_sim_arm_library
  ios_sim_architectures=arm64
  if [[ "$mode" == universal ]]; then
    echo "== build x86_64 iOS Simulator Rust library =="
    IPHONEOS_DEPLOYMENT_TARGET=$ios_deployment_target \
      cargo build -p "$crate" --locked --release --target "$ios_sim_x86_target"
    ios_sim_x86_library=$target_dir/$ios_sim_x86_target/release/$library

    mkdir -p "$staging/ios-simulator"
    ios_sim_library=$staging/ios-simulator/$library
    lipo -create \
      "$ios_sim_arm_library" \
      "$ios_sim_x86_library" \
      -output "$ios_sim_library"
    ios_sim_architectures="arm64 x86_64"
  fi
fi

echo "== generate UniFFI Swift source and C module =="
rm -rf "$generated" "$headers"
mkdir -p "$generated" "$headers" "$swift_sources"
cargo run -p "$crate" --locked --features bindgen --bin uniffi-bindgen -- generate \
  --library "$arm_library" \
  --language swift \
  --out-dir "$generated"
# UniFFI 0.29 emits trailing blanks on otherwise empty/generated lines. Strip
# only horizontal end-of-line whitespace so checked-in bindings remain
# deterministic and pass the repository diff gate without changing semantics.
perl -pi -e 's/[ \t]+$//' "$generated/nmp_native_runtime_ffi.swift"
if [[ "$check_bindings" == true ]]; then
  if [[ ! -f "$packaged_swift" ]]; then
    echo "error: checked-in Swift binding is missing: $packaged_swift" >&2
    exit 1
  fi
  if ! cmp -s "$generated/nmp_native_runtime_ffi.swift" "$packaged_swift"; then
    echo "error: generated Swift binding is stale" >&2
    echo "run scripts/build-runtime-swift-xcframework.sh --universal and commit the Swift source" >&2
    exit 1
  fi
fi
cp "$generated/nmp_native_runtime_ffiFFI.h" "$headers/"
cp "$generated/nmp_native_runtime_ffiFFI.modulemap" "$headers/module.modulemap"
cp "$generated/nmp_native_runtime_ffi.swift" \
  "$packaged_swift"

xcframework_libraries=(-library "$packaged_library" -headers "$headers")
xcframework_platforms="macOS ($architectures)"
if [[ "$ios_enabled" == true ]]; then
  xcframework_libraries+=(-library "$ios_device_library" -headers "$headers")
  xcframework_libraries+=(-library "$ios_sim_library" -headers "$headers")
  xcframework_platforms+=", iOS ($ios_device_target), iOS Simulator ($ios_sim_architectures)"
fi

echo "== create XCFramework ($xcframework_platforms) =="
rm -rf "$xcframework"
xcodebuild -create-xcframework \
  "${xcframework_libraries[@]}" \
  -output "$xcframework"

echo "== validate packaged architectures and generated binding =="
validate_architectures() {
  local label=$1
  local archive=$2
  shift 2
  local expected_architectures=("$@")
  local actual_architectures=()
  local expected

  read -r -a actual_architectures <<<"$(lipo -archs "$archive")"
  for expected in "${expected_architectures[@]}"; do
    if [[ ! " ${actual_architectures[*]} " =~ [[:space:]]${expected}[[:space:]] ]]; then
      echo "error: $label library is missing architecture $expected" >&2
      exit 1
    fi
  done
  if [[ "${#actual_architectures[@]}" -ne "${#expected_architectures[@]}" ]]; then
    echo "error: $label library has unexpected architectures: ${actual_architectures[*]}" >&2
    exit 1
  fi
  printf '%s: %s\n' "$label" "${actual_architectures[*]}"
}

expected_architectures=(arm64)
if [[ "$mode" == universal ]]; then
  expected_architectures+=(x86_64)
fi
validate_architectures "macOS" "$packaged_library" "${expected_architectures[@]}"
if [[ "$ios_enabled" == true ]]; then
  test -s "$ios_device_library"
  test -s "$ios_sim_library"
  validate_architectures "iOS device" "$ios_device_library" arm64
  validate_architectures "iOS Simulator" "$ios_sim_library" "${expected_architectures[@]}"
fi
test -s "$packaged_swift"
test -s "$xcframework/Info.plist"

echo "XCFramework: $xcframework"
echo "Swift source: $packaged_swift"
