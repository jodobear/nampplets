#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH='' cd -- "$script_dir/../.." && pwd)
build_script=$repo_root/scripts/build-runtime-swift-xcframework.sh

bash -n "$build_script"
help=$("$build_script" --help)
grep -Fq -- "--arm64-only" <<<"$help"
grep -Fq -- "--universal" <<<"$help"
grep -Fq -- "--no-ios" <<<"$help"
grep -Fq -- "--check-bindings" <<<"$help"

test_root=$(mktemp -d)
trap 'rm -rf "$test_root"' EXIT
error_file=$test_root/error.log
if "$build_script" --unknown >"$error_file" 2>&1; then
  echo "unknown option unexpectedly succeeded" >&2
  exit 1
fi
grep -Fq "unknown option: --unknown" "$error_file"

fixture_repo=$test_root/repo
fixture_bin=$test_root/bin
fixture_target=$test_root/target
tool_log=$test_root/tools.log
mkdir -p \
  "$fixture_repo/scripts" \
  "$fixture_repo/Packages/NMPNativeRuntime/Sources/NMPNativeRuntime" \
  "$fixture_bin"
cp "$build_script" "$fixture_repo/scripts/"
printf '%s\n' '// generated binding' \
  > "$fixture_repo/Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/NMPNativeRuntime.swift"

cat >"$fixture_bin/cargo" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf 'cargo %s\n' "$*" >>"$TOOL_LOG"
case "${1:-}" in
  build)
    target=
    while (($#)); do
      if [[ "$1" == --target ]]; then
        target=$2
        break
      fi
      shift
    done
    test -n "$target"
    mkdir -p "$CARGO_TARGET_DIR/$target/release"
    printf '%s\n' "$target archive" \
      > "$CARGO_TARGET_DIR/$target/release/libnmp_native_runtime_ffi.a"
    ;;
  run)
    output=
    while (($#)); do
      if [[ "$1" == --out-dir ]]; then
        output=$2
        break
      fi
      shift
    done
    test -n "$output"
    mkdir -p "$output"
    printf '%s\n' '// generated binding' \
      > "$output/nmp_native_runtime_ffi.swift"
    printf '%s\n' '// generated header' \
      > "$output/nmp_native_runtime_ffiFFI.h"
    printf '%s\n' 'module generated {}' \
      > "$output/nmp_native_runtime_ffiFFI.modulemap"
    ;;
  *)
    echo "unexpected cargo invocation" >&2
    exit 1
    ;;
esac
MOCK

cat >"$fixture_bin/lipo" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf 'lipo %s\n' "$*" >>"$TOOL_LOG"
if [[ "${1:-}" == -create ]]; then
  output=
  while (($#)); do
    if [[ "$1" == -output ]]; then
      output=$2
      break
    fi
    shift
  done
  test -n "$output"
  printf '%s\n' 'universal archive' >"$output"
elif [[ "${1:-}" == -archs ]]; then
  if [[ -n "${MOCK_LIPO_ARCHS:-}" ]]; then
    printf '%s\n' "$MOCK_LIPO_ARCHS"
  elif [[ "$2" == *runtime-swift-package/ios-simulator* \
    && -n "${MOCK_IOS_SIM_LIPO_ARCHS:-}" ]]; then
    printf '%s\n' "$MOCK_IOS_SIM_LIPO_ARCHS"
  elif [[ "$2" == *aarch64-apple-darwin* \
    || "$2" == *aarch64-apple-ios/release* \
    || "$2" == *aarch64-apple-ios-sim* ]]; then
    printf '%s\n' arm64
  else
    printf '%s\n' 'arm64 x86_64'
  fi
else
  echo "unexpected lipo invocation" >&2
  exit 1
fi
MOCK

cat >"$fixture_bin/xcodebuild" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
printf 'xcodebuild %s\n' "$*" >>"$TOOL_LOG"
output=
while (($#)); do
  if [[ "$1" == -output ]]; then
    output=$2
    break
  fi
  shift
done
test -n "$output"
mkdir -p "$output"
printf '%s\n' '<?xml version="1.0"?>' >"$output/Info.plist"
MOCK

chmod +x "$fixture_bin/cargo" "$fixture_bin/lipo" "$fixture_bin/xcodebuild"

run_fixture() {
  PATH="$fixture_bin:$PATH" \
    TOOL_LOG="$tool_log" \
    CARGO_TARGET_DIR="$fixture_target" \
    "$fixture_repo/scripts/build-runtime-swift-xcframework.sh" "$@"
}

: >"$tool_log"
run_fixture --universal --check-bindings
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target aarch64-apple-darwin' "$tool_log"
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target x86_64-apple-darwin' "$tool_log"
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target aarch64-apple-ios' "$tool_log"
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target aarch64-apple-ios-sim' "$tool_log"
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target x86_64-apple-ios' "$tool_log"
grep -Fq 'lipo -create '"$fixture_target"'/aarch64-apple-ios-sim/release/libnmp_native_runtime_ffi.a '"$fixture_target"'/x86_64-apple-ios/release/libnmp_native_runtime_ffi.a -output '"$fixture_target"'/runtime-swift-package/ios-simulator/libnmp_native_runtime_ffi.a' "$tool_log"
test -s "$fixture_repo/Packages/NMPNativeRuntime/NMPNativeRuntime.xcframework/Info.plist"

if MOCK_IOS_SIM_LIPO_ARCHS=arm64 \
  run_fixture --universal >"$error_file" 2>&1; then
  echo "incomplete iOS Simulator architecture set was accepted" >&2
  exit 1
fi
grep -Fq 'iOS Simulator library is missing architecture x86_64' "$error_file"

printf '%s\n' '// stale binding' \
  > "$fixture_repo/Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/NMPNativeRuntime.swift"
if run_fixture --arm64-only --check-bindings >"$error_file" 2>&1; then
  echo "stale checked-in binding unexpectedly succeeded" >&2
  exit 1
fi
grep -Fq 'generated Swift binding is stale' "$error_file"
grep -Fq '// stale binding' \
  "$fixture_repo/Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/NMPNativeRuntime.swift"

: >"$tool_log"
run_fixture --arm64-only
grep -Fq 'cargo build -p nmp-native-runtime-ffi --locked --release --target aarch64-apple-darwin' "$tool_log"
if grep -Fq 'x86_64-apple-darwin' "$tool_log"; then
  echo "arm64-only build unexpectedly requested x86_64" >&2
  exit 1
fi
if grep -Fq 'x86_64-apple-ios' "$tool_log"; then
  echo "arm64-only build unexpectedly requested x86_64 iOS Simulator" >&2
  exit 1
fi
if grep -Fq 'lipo -create ' "$tool_log"; then
  echo "arm64-only build unexpectedly created a universal archive" >&2
  exit 1
fi
grep -Fq '// generated binding' \
  "$fixture_repo/Packages/NMPNativeRuntime/Sources/NMPNativeRuntime/NMPNativeRuntime.swift"

if MOCK_LIPO_ARCHS='arm64 x86_64' run_fixture --arm64-only >"$error_file" 2>&1; then
  echo "unexpected architecture set was accepted" >&2
  exit 1
fi
grep -Fq 'macOS library has unexpected architectures' "$error_file"

: >"$tool_log"
run_fixture --arm64-only --no-ios
if grep -Fq 'apple-ios' "$tool_log"; then
  echo "--no-ios unexpectedly requested an iOS build" >&2
  exit 1
fi

echo "build-runtime-swift-xcframework script contract passed"
