# CavalRe Ledger

A reusable core based on the original Solidity Ledger, with a Solana adapter.
Both share one implementation of the accounting rules.

The program now supports ledger creation, account registration and removal,
implicit leaves, transfers, and standard SPL Token deposits and withdrawals.
The reusable core implements the original depth-aligned debit/credit walk.
This is a tested development implementation, not a deployed or audited release.

Preserve the original accounting and recognizable module structure while
adapting storage, addresses and authorization to Solana. Standalone Solidity
work is deferred until this implementation settles.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/cavalre-ledger-core/src/ledger_lib.rs` | Shared LedgerLib rules; no platform dependencies |
| `crates/cavalre-ledger-solana/src/ledger_lib.rs` | Solana PDA derivation and calls into the core |
| `crates/cavalre-ledger-solana/src/ledger.rs` | Authenticated program operations and storage |
| `crates/cavalre-ledger-solana/src/ledger_view.rs` | Read helpers |
| `tests/runtime/`, `tests/consumer/` | Actual sBPF execution and application CPI tests |
| `crates/*/tests/` | Platform-independent behavior and Solana adapter tests |
| `docs/PORTING.md` | Source baseline, module mapping and reusable work in Git history |
| `reference/`, `tests/reference/`, `spec/fixtures/` | Original Solidity reference and fixture generation |
| `scripts/` | Validation, pinned tool installation and reference tooling |

The previous controller-based implementation is removed from the working tree.
Its code and tests remain available in Git history.

The core is `no_std` with no dependencies. Hosts provide deterministic address
derivation and authenticated state through the core interfaces. The Solana
adapter supplies public keys and PDAs; other hosts can supply their own address
types and storage. No separate kernel crate is needed at this stage.

## Verify

Host Rust is pinned to **1.98.1**, including rustfmt and Clippy.

```bash
bash scripts/install-agave.sh
export PATH="$PWD/target/toolchains/solana-release/bin:$PATH"
bash scripts/check.sh
```

The check formats, lints, builds actual program and test-consumer sBPF artifacts,
and runs core and runtime tests. Agave is pinned to **4.3.0** and platform-tools
to **v1.57**. Stack-limit diagnostics fail the build.

The posting regression replays all **162** saved Solidity hierarchy cases and
checks every resulting balance and rejection. Runtime tests exercise internal
issuance, implicit receipts, parent admission, account lifecycle, token custody,
application isolation, PDA authority, and transaction rollback. This is not the
full original Solidity test suite or a security audit.

See [program usage and limitations](crates/cavalre-ledger-solana/README.md)
before attempting deployment. The checked-in program ID is for local simulation.

## Original reference

The Solidity reference and saved accounting fixtures are comparison material,
not an implementation dependency. To reproduce those fixtures with Foundry **1.8.3**:

```bash
git submodule update --init reference/cavalre-contracts
bash scripts/install-foundry.sh
export PATH="$PWD/target/toolchains/foundry:$PATH"
bash scripts/reference.sh
```

See the [core README](crates/cavalre-ledger-core/README.md) and
[Solana README](crates/cavalre-ledger-solana/README.md) for implemented
behavior. Publication and deployment require an explicit release decision.
