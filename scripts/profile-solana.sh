#!/usr/bin/env bash
# Rebuild real sBPF, check sampled transaction budgets, and print a compact report.
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/build-sbf.sh tests/consumer/Cargo.toml
bash scripts/build-sbf.sh crates/cavalre-ledger-solana/Cargo.toml
cargo test -p cavalre-ledger-runtime-tests execution_limits --locked
sha256sum target/deploy/cavalre_ledger_solana.so target/deploy/cavalre_ledger_test_consumer.so
python3 - <<'PY'
import json

with open("target/execution-profile.json") as stream:
    rows = json.load(stream)
assert rows and all(row["status"] == "ok" for row in rows)
v0 = [row for row in rows if row["format"] == "v0"]
print(f"\n{len(rows)} measured transactions; {len(rows)-len(v0)} signed legacy, "
      f"{len(v0)} signed v0/lookup-table packets; 300000 CU budget.")
print(f"Largest legacy encoding: {max(row['legacy_bytes'] for row in rows)} bytes.")
print("Lookup table setup is measured separately; aggregate fees "
      f"{sum(row['lookup_setup_fee_lamports'] for row in rows)} lamports, "
      f"storage funding {sum(row['lookup_setup_rent_lamports'] for row in rows)} lamports.")
print("\n| Leaf depth | Max CU | Max bytes | Max account keys | Max writable keys |")
print("| --- | --- | --- | --- | --- |")
for depth in sorted({row["depth"] for row in rows}):
    group = [row for row in rows if row["depth"] == depth]
    values = [max(row[key] for row in group)
              for key in ("compute_units", "bytes", "accounts", "writable")]
    print("| " + " | ".join(map(str, [depth, *values])) + " |")
depth = max(row["depth"] for row in rows)
print(f"\nOperations at leaf depth {depth}; maxima across modes and seeds:")
print("\n| Operation | Max CU | Max bytes | New storage funding (SOL) |")
print("| --- | --- | --- | --- |")
for operation in sorted({row["operation"] for row in rows}):
    group = [row for row in rows if row["operation"] == operation and row["depth"] == depth]
    rent = sorted({row["rent_lamports"] / 1e9 for row in group})
    print(f"| {operation} | {max(row['compute_units'] for row in group)} | "
          f"{max(row['bytes'] for row in group)} | " + ", ".join(f"{r:g}" for r in rent) + " |")
print("\nDetailed samples: target/execution-profile.json")
PY
