# CavalRe Ledgers

Hierarchical double-entry accounting and omnibus custody, with a portable Rust
service, a Solana adapter, and a standalone Solidity implementation.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/` | Rust accounting kernel, portable service, and Solana adapter |
| `contracts/` | Solidity service and isolated ERC20 custody vaults |
| `tests/solidity/` | EVM controller, accounting, custody and shared-fixture tests |
| `tests/integration/`, `tests/solana-consumer/` | Solana runtime and consumer tests |
| `docs/` | Shared behavioral specifications and platform-specific documentation |
| `spec/fixtures/` | Shared accounting and custody reference fixtures |
| `tests/reference/`, `reference/` | Isolated Solidity fixture generator and pinned source |

Rust and Solidity have separate build tooling. `Cargo.toml` and `foundry.toml`
live at the repository root; generated output stays under ignored `target/`.

## Rust crates

| Crate | Responsibility |
| --- | --- |
| `cavalre-ledgers-kernel` | Checked posting arithmetic and hierarchy validation; no authorization or storage |
| `cavalre-ledgers-core` | Controller permissions, namespace/account lifecycle, journal and claim transfers, custody accounting |
| `cavalre-ledgers-solana` | Solana program and CPI interface, authenticated accounts, native tokens and atomic execution |

The core depends only on the accounting kernel. Both are `no_std` with `alloc`.
The Solana adapter depends on the core; portable consumers do not need Anchor
or a Solana SDK.

## Use the core

```rust
use cavalre_ledgers_core::{Authorization, RootKind, create_root, initialize_namespace};

fn main() -> Result<(), cavalre_ledgers_core::Error> {
    let creator = [1; 32]; // identity already authenticated by the host
    let signers = [creator];
    let auth = Authorization::from_verified_signers(&signers);
    let namespace = initialize_namespace([2; 32], creator, auth)?;
    let journal = create_root(&namespace, [3; 32], RootKind::Journal, auth)?;
    // Persist the namespace and journal atomically in the host's storage.
    Ok(())
}
```

The host loads current authenticated records and atomically applies successful
plans. See [the complete adapter contract](docs/LEDGER_CORE.md) before integrating
storage or transaction authorization. The arithmetic kernel alone grants no
spending permissions.

For Solana clients, depend on `cavalre-ledgers-solana` with `cpi` for Anchor CPI
or `no-entrypoint` for instruction/account types. Its Rust import is
`cavalre_ledgers_solana`; the built program is `cavalre_ledgers_solana.so`.
The simulation program ID, instruction discriminators and account layouts are
unchanged by the extraction and package rename.

## Build and verify

### Rust and Solana

Host Rust is pinned to **1.98.1**. The default workspace members are the portable
crates, so these commands require no Solana toolchain:

```bash
cargo test --locked
cargo clippy -p cavalre-ledgers-kernel -p cavalre-ledgers-core --all-targets --locked -- -D warnings
```

To verify the Solana adapter, install pinned Agave **4.3.0** and run the full
gate. It compiles actual sBPF artifacts with platform-tools **v1.57**, tests the
external application consumer, and runs the host, runtime and release-helper
suites:

```bash
bash scripts/install-agave.sh
export PATH="$PWD/target/toolchains/solana-release/bin:$PATH"
bash scripts/check.sh
```

### Solidity

The EVM implementation uses Foundry **1.8.3**, Solidity **0.8.26**, and **Cancun**.
From the repository root:

```bash
bash scripts/install-foundry.sh
export PATH="$PWD/target/toolchains/foundry:$PATH"
bash scripts/check-solidity.sh
```

`Ledgers` implements namespaces, per-account controllers, hierarchical journals,
and ERC20 claims. Controllers authorize through calls; joint actions use expiring,
single-use consent for the exact operation. `LedgerVault` isolates each
namespace's collateral for each token. See the [EVM interface](docs/LEDGER_EVM.md)
for authority rules, examples and token assumptions.

The Solidity suite replays all 300 shared reference actions and tests controller
isolation, approval expiry and replay, contract callers, bounded arithmetic,
custody settlement and rollback. Artifacts and cache stay under
`target/solidity/`. Rust/Solana and Solidity checks run independently in CI.

### Shared reference fixtures

The fixture baseline contains 162 hierarchy and 138 custody actions from pinned
Solidity code. Reproduce it with Foundry **1.8.3**:

```bash
git submodule update --init reference/cavalre-contracts
bash scripts/reference.sh
```

The reference checkout is read-only specification material. Successful tests
are not an audit or deployment. All test consumers have `publish = false` and
must never be deployed.

## Documentation and releases

- [Portable core and host adapter contract](docs/LEDGER_CORE.md)
- [Accounting model](docs/ACCOUNTING.md)
- [EVM controllers, journals and custody](docs/LEDGER_EVM.md)
- [Solana custody, controller and token policy](docs/OMNIBUS_LEDGER.md)
- [Solana release and deployment preparation](docs/LEDGER_MAINNET.md)

Crates.io publication is disabled pending an explicit release and license
selection. Internal package dependencies include versions and local paths;
consumers can pin this public repository by Git revision in the meantime.
Fetching that dependency requires no GitHub credentials or repository secrets.
