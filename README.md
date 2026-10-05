# CavalRe Ledger

A reusable core based on the original Solidity Ledger, with a Solana adapter.
Both share one implementation of the accounting rules.

The program now supports ledger creation, account registration and removal,
implicit leaves, transfers, and native SOL, classic SPL Token and compatible
Token-2022 deposits and withdrawals.
The reusable core implements the original depth-aligned debit/credit walk.
This is a tested development implementation, not a deployed or audited release.

Preserve the original accounting and recognizable module structure while
adapting storage, addresses and authorization to Solana. Standalone Solidity
work is deferred until this implementation settles.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/cavalre-ledger-core/src/ledger_lib.rs` | Original LedgerLib identity and posting rules |
| `crates/cavalre-ledger-core/src/ledger.rs` | Shared service operations and authenticated host interface |
| `crates/cavalre-ledger-core/src/ledger_view.rs` | Independent, non-mutating queries |
| `crates/cavalre-ledger-solana/src/ledger_lib.rs` | Solana PDA derivation and calls into the core |
| `crates/cavalre-ledger-solana/src/ledger.rs` | Solana implementation of the host interface |
| `crates/cavalre-ledger-solana/src/ledger_view.rs` | Read helpers |
| `crates/cavalre-ledger-solana/src/ledger_cpi.rs` | In-place custody instruction preparation |
| `tests/runtime/`, `tests/consumer/` | Actual sBPF execution and application CPI tests |
| `crates/*/tests/` | Platform-independent behavior and Solana adapter tests |
| `docs/PORTING.md` | Source baseline, module mapping and reusable work in Git history |
| `reference/`, `tests/reference/`, `spec/fixtures/` | Original Solidity reference and fixture generation |
| `scripts/` | Validation, pinned tool installation and reference tooling |

The previous controller-based implementation is removed from the working tree.
Its code and tests remain available in Git history.

The core is `no_std` with no runtime dependencies. Its service interface requires
host authentication, address derivation, state access, native token movement and
atomic commit. The core enforces custodian permissions, account lifecycle,
implicit-leaf admission, posting, backing and exact settlement. The Solana adapter
implements those host capabilities using runtime signers, PDAs, program accounts
and token calls. See the [core host contract](crates/cavalre-ledger-core/README.md).
No separate kernel crate is needed at this stage.

The original Ledger/LedgerView separation is preserved. Both crates can be built
with `default-features = false` to exclude the mutating `ledger` module while
retaining queries over stored state. Reads require neither the mutation host nor
a signer, payer, token call or enabled mutation dispatcher. See the
[read interface](docs/READS.md).

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

The suite contains **120 test functions**: **36 core tests**, **24 Solana library
tests**, and **60 sBPF runtime tests**. The read suites
also run with mutation modules excluded; these repeat runs are not extra tests.
One core test replays the 162 saved posting cases; those are not 162 separate
test functions. See [acceptance coverage](docs/ACCEPTANCE.md) for the exercised
requirements and remaining validation work.

Ledger imposes no resource-based depth cap. Applications are responsible for
fitting their complete transactions within runtime budgets. See
[execution measurements](docs/EXECUTION_LIMITS.md) for compute, transaction sizes,
storage funding and reproduction instructions.

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
