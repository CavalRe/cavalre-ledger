# Porting the original Ledger to Solana

The behavioral baseline is `cavalre-contracts/modules/ledger/LedgerLib.sol` at
[`34d159f`](https://github.com/CavalRe/cavalre-contracts/tree/34d159ff4e88fdfdee16738d9a1228f0bf407212/modules/ledger).
Use that implementation to determine accounting behavior. The separately pinned
reference fixture generator remains unchanged and is comparison material, not
proof of complete parity with this baseline.

| Solidity module | Rust module | Status |
| --- | --- | --- |
| `LedgerLib.sol` | `cavalre-ledger-core/src/ledger_lib.rs` | Identity, effective flags, custody lookup and posting walk implemented |
| `Ledger.sol` | Core `ledger.rs`, plus the Solana host adapter | Shared account management, permissions, transfers and settlement policy implemented |
| `LedgerView.sol` | Core and Solana `ledger_view.rs` | Independent account, balance, custody, registered-child queries, including ledger discovery through Root; metadata differences documented in READS.md |
| `ILedger` / `LedgerLib` events | Core `ledger::Event`, host `emit`, Solana Anchor events | Original structural and posting event families; payload adaptations and transaction-status requirements in EVENTS.md |

The reusable core holds shared rules and the `Host` interface without platform dependencies.
`cavalre-ledger-solana/src/ledger_lib.rs` supplies PDA derivation and forwards to
the core. Address types are host-defined; the core does not impose Solana keys.
The pure accounting portion remains in the core rather than a separate
kernel crate for now. Core `ledger::execute` requires host authentication and
transactional storage/settlement, and enforces lifecycle and custodian policy.
The Solana `ledger.rs` implements those capabilities. Global Root is a canonical
PDA using the same 512-byte record format as other accounts. Its children are
ledger groups; discovery uses the shared child queries, with Solana's current
address-order enumeration documented in READS.md. Creation updates Root's child
count atomically; ordinary posting still stops at the selected depth-2 ledger.

The `mutations` feature controls each crate's mutating `ledger` module. Queries
and shared account types remain available with that feature disabled. Record
encoding/decoding is shared in the Solana `ledger_lib.rs`; readers do not depend
on or invoke the mutation program. This preserves the original ability to
remove mutation dispatch while retaining access to stored state.

Keep corresponding functions in recognizable order and use Rust naming
conventions. Introduce platform-specific files only for actual Solana needs.
There are no placeholder program modules implying unfinished features work.

## Recover useful work without restoring the old model

The superseded implementation is preserved at
[`37e46c5`](https://github.com/CavalRe/cavalre-ledger/tree/37e46c5b9465cdf9a501d99eea5b661c0914834a).
The paths below refer to that commit, not the current working tree.

| When needed | Prior source | What to adapt |
| --- | --- | --- |
| Token custody | `crates/cavalre-ledgers-solana/src/tokens.rs`, `src/lib.rs` | Mint/program/authority checks, exact settlement and full backing checks |
| Account loading | `crates/cavalre-ledgers-solana/src/hierarchy.rs` | Ownership, PDA, discriminator and ancestry validation |
| Token runtime tests | `tests/integration/omnibus_tokens.rs`, `custody_shared.rs` | Real token execution, failed-transfer rollback and custody invariants |
| Authorization tests | `tests/integration/omnibus_authority.rs`, `tests/solana-consumer/` | CPI signer plumbing and rejection tests, rewritten for custodian permissions |
| Posting runtime tests | `tests/integration/hierarchy.rs` | Malformed account paths, ancestor balances and atomic rollback |

Review the token-extension policy before carrying it over; the old policy is
not automatically the new product policy. Validate all recovered checks against
the new account layout and permissions. Do not preserve test expectations for
per-leaf controllers, mandatory position registration, separate namespace token
trees or an implicit Source.

The original `transfer` ancestor walk is the source for the new accounting
implementation. The old Rust arithmetic can help compare results, but does not
define the new structure or permissions.

Rust and Agave pins, the sBPF build/stack checks, and the original Solidity
reference tooling remain in the working tree. Old deployment scripts and
program identities were removed because they target the superseded program.
