#!/usr/bin/env bash
# Use once the draft has an executable program; this does not deploy it.
set -euo pipefail
cd "$(dirname "$0")/.."
manifest="${1:?Usage: bash scripts/build-sbf.sh <program/Cargo.toml>}"
[[ "$(cargo-build-sbf --version | head -n 1)" == 'cargo-build-sbf 4.3.0' ]] || {
  echo 'Expected cargo-build-sbf 4.3.0; see scripts/install-agave.sh.' >&2
  exit 1
}
mkdir -p target
report="$PWD/target/sbf-build.log"
cargo build-sbf --manifest-path "$manifest" --tools-version v1.57 \
  -j "${CARGO_BUILD_JOBS:-4}" -- --locked 2>&1 | tee "$report"
if grep -Ei 'overflows the maximum allowed frame|stack offset.*exceeded' "$report"; then
  echo 'sBPF stack limit exceeded' >&2
  exit 1
fi
