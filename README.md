# CavalRe Ledger

A reusable core based on the original Solidity Ledger, with a Solana adapter.
Both share one implementation of the accounting rules.

The draft currently implements account identity, inherited effective flags,
ledger lookup and custody resolution in `crates/cavalre-ledger-core/src/ledger_lib.rs`, corresponding to
`cavalre-contracts/modules/ledger/LedgerLib.sol`. Transfers, persistent account
storage and program instructions are still to come. It is not deployable yet.

Preserve the original accounting and recognizable module structure while
adapting storage, addresses and authorization to Solana. Standalone Solidity
work is deferred until this implementation settles.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/cavalre-ledger-core/src/ledger_lib.rs` | Shared LedgerLib rules; no platform dependencies |
| `crates/cavalre-ledger-solana/src/ledger_lib.rs` | Solana PDA derivation and calls into the core |
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
bash scripts/check.sh
```

This formats-checks, lints and tests the active workspace. It does not claim to
run a Solana program: no executable program exists in this draft yet.

The pinned Agave **4.3.0** installer and sBPF build helper remain for the program
stage. The helper uses platform-tools **v1.57** and rejects stack-limit warnings:

```bash
bash scripts/install-agave.sh
export PATH="$PWD/target/toolchains/solana-release/bin:$PATH"
# Once an executable program exists:
# bash scripts/build-sbf.sh <program/Cargo.toml>
```

## Original reference

The Solidity reference and saved accounting fixtures are comparison material,
not an implementation dependency. The draft's current tests do not yet replay
the saved posting fixtures. To reproduce those fixtures with Foundry **1.8.3**:

```bash
git submodule update --init reference/cavalre-contracts
bash scripts/install-foundry.sh
export PATH="$PWD/target/toolchains/foundry:$PATH"
bash scripts/reference.sh
```

See the [core README](crates/cavalre-ledger-core/README.md) and
[Solana README](crates/cavalre-ledger-solana/README.md) for implemented
behavior. Publication and deployment require an explicit release decision.
