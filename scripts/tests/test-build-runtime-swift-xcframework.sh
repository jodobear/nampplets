#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
build_script=$repo_root/scripts/build-runtime-swift-xcframework.sh

bash -n "$build_script"
help=$("$build_script" --help)
grep -Fq -- "--arm64-only" <<<"$help"
grep -Fq -- "--universal" <<<"$help"

error_file=$(mktemp)
trap 'rm -f "$error_file"' EXIT
if "$build_script" --unknown >"$error_file" 2>&1; then
  echo "unknown option unexpectedly succeeded" >&2
  exit 1
fi
grep -Fq "unknown option: --unknown" "$error_file"

echo "build-runtime-swift-xcframework script contract passed"
