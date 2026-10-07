#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
# Separate invocations avoid workspace feature unification enabling mutations.
cargo test -p cavalre-ledger-core --no-default-features --locked
cargo test -p cavalre-ledger-solana --no-default-features --locked
bash scripts/build-sbf.sh tests/consumer/Cargo.toml
bash scripts/build-sbf.sh crates/cavalre-ledger-solana/Cargo.toml
bash scripts/build-sbf.sh experiments/indexed-ledger/Cargo.toml
cargo test --workspace --locked
