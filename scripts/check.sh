#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "$(cargo-build-sbf --version | head -n 1)" != "cargo-build-sbf 4.3.0" ]]; then
  echo 'Expected cargo-build-sbf 4.3.0; see README.md.' >&2
  exit 1
fi
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
python3 -B scripts/ledger/test_mainnet.py
mkdir -p target
build_sbpf() {
  local manifest="$1"
  local report="$2"
  cargo build-sbf --manifest-path "$manifest" --tools-version v1.57 \
    -j "${CARGO_BUILD_JOBS:-4}" -- --locked 2>&1 | tee "$report"
  if grep -Ei 'overflows the maximum allowed frame|stack offset.*exceeded' "$report"; then
    echo 'sBPF stack limit exceeded' >&2
    return 1
  fi
}
build_sbpf tests/solana-consumer/Cargo.toml target/ledger-consumer-build.log
build_sbpf crates/ledger-solana/Cargo.toml target/ledger-build.log
CAVALRE_LEDGER_COST_REPORT="$PWD/target/custody-costs.json" \
CAVALRE_LEDGER_HIERARCHY_REPORT="$PWD/target/hierarchy-costs.json" cargo test --workspace --locked
