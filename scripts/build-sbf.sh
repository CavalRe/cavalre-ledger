#!/usr/bin/env bash
# Build an executable program and reject stack-limit diagnostics; never deploy.
set -euo pipefail
cd "$(dirname "$0")/.."
manifest="${1:?Usage: bash scripts/build-sbf.sh <program/Cargo.toml>}"
[[ "$(cargo-build-sbf --version | head -n 1)" == 'cargo-build-sbf 4.3.0' ]] || {
  echo 'Expected cargo-build-sbf 4.3.0; see scripts/install-agave.sh.' >&2
  exit 1
}
mkdir -p target
report="$PWD/target/sbf-build.log"
features=()
if [[ "$manifest" == "crates/cavalre-ledger-solana/Cargo.toml" ]]; then
  features=(--features p-token-entrypoint)
fi
cargo build-sbf "${features[@]}" --manifest-path "$manifest" --tools-version v1.57 \
  -j "${CARGO_BUILD_JOBS:-4}" -- --locked 2>&1 | tee "$report"
if grep -Ei 'overflows the maximum allowed frame|stack offset.*exceeded' "$report"; then
  echo 'sBPF stack limit exceeded' >&2
  exit 1
fi
