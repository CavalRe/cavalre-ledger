# CavalRe Ledger

Hierarchical double-entry accounting and omnibus custody, with a portable Rust
service and a Solana adapter.

| Crate | Responsibility |
| --- | --- |
| `cavalre-accounting` | Checked posting arithmetic and hierarchy validation; no authorization or storage |
| `cavalre-ledger-core` | Controller permissions, namespace/account lifecycle, journal and claim transfers, custody accounting |
| `cavalre-ledger-solana` | Solana program and CPI interface, authenticated accounts, native tokens and atomic execution |

The core depends only on the accounting kernel. Both are `no_std` with `alloc`.
The Solana adapter depends on the core; portable consumers do not need Anchor
or a Solana SDK. SR, Multiswap economics and validator admission are consuming
applications, not Ledger dependencies.

## Use the core

```rust
use cavalre_ledger_core::{Authorization, RootKind, create_root, initialize_namespace};

fn main() -> Result<(), cavalre_ledger_core::Error> {
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

For Solana clients, depend on `cavalre-ledger-solana` with `cpi` for Anchor CPI
or `no-entrypoint` for instruction/account types. Its Rust import is
`cavalre_ledger_solana`; the built program is `cavalre_ledger_solana.so`.
The simulation program ID, instruction discriminators and account layouts are
unchanged by the extraction and package rename.

## Build and verify

Host Rust is pinned to **1.98.1**. The default workspace members are the portable
crates, so these commands require no Solana toolchain:

```bash
cargo test --locked
cargo clippy -p cavalre-accounting -p cavalre-ledger-core --all-targets --locked -- -D warnings
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
- [Solana custody, controller and token policy](docs/OMNIBUS_LEDGER.md)
- [Solana release and deployment preparation](docs/LEDGER_MAINNET.md)

The implementation was extracted from `CavalRe/cavalre-solana` at
`cdd98ab33f472ec289bda88384511253e9f3c063`; source pins are recorded in
`spec/upstream.json`. The older bundled Ledger/SR prototype and Multiswap
applications remain in that repository and consume the shared accounting crate.

Crates.io publication is disabled pending an explicit release and license
selection. Internal package dependencies include versions and local paths;
consumers can pin this public repository by Git revision in the meantime.
Fetching that dependency requires no GitHub credentials or repository secrets.
