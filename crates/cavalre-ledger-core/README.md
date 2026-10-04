# Ledger core

Platform-independent rules based on the original Solidity `LedgerLib.sol` at
`34d159ff4e88fdfdee16738d9a1228f0bf407212`. This crate is `no_std` with no runtime
dependencies. It preserves the existing relative identities, custody ancestry,
implicit leaves, Source and posting walk.

`src/ledger_lib.rs` retains the original names and responsibilities in Rust
style: account kinds, flags, `effective_flags`, `ledger` and `custody`.
Registered metadata takes precedence; implicit leaves inherit their parent's
polarity and depth without registration. Custody resolution identifies an
account's custodian; it does not authorize spending.

## Service and host boundary

`src/ledger.rs` corresponds to the public responsibilities of `Ledger.sol`.
`execute(host, command)` implements root/Source initialization, account creation,
registration and removal, transfers, deposits and withdrawals. It owns name,
kind, balance, child-count, parent admission and custodian permission checks,
including repeated and no-op operations. It verifies exact native settlement
and full-liability backing before withdrawals.

The `Host<A>` interface is defined by the core. A host supplies an address type
(`Copy + Eq`) and the following capabilities:

| Method | Host responsibility |
| --- | --- |
| `root` | Authenticate the selected root, asset/identifier, reserved Source identity and accounting-only owner. |
| `authenticate(role, command)` | Establish the acting authority or token payer from the current execution context. Verify signatures or consume the runtime's verified result. |
| `to_address` | Derive a child identity from its absolute parent and relative identifier in this service's domain. |
| `account`, `put` | Read authenticated logical state and stage writes without imposing a serialization or database format. |
| `token_balances`, `move_tokens` | Validate supported native assets, bind the vault and wallet to this operation, observe balances and execute native movement. |
| `atomic`, `commit` | Commit storage and token effects together, or roll all effects back on any error. |

The acting authority can be an application identity distinct from the transaction
signer and fee payer. A deposit additionally authenticates the token payer; the
core requires that identity to own the funding wallet. Withdrawals do not ask
for recipient authentication. A supplied address is never proof of authority.

Authentication receives the command so a host can bind verification to the
requested operation. It must also bind the root, asset, wallet, service and chain
from the host context. This interface does not introduce an intent protocol or
require duplicate signature verification after a runtime has already done it.

The host is trusted integration code, not a user-provided callback. It must
validate ownership, identities, root membership and malformed state before
exposing records. Core account values are logical projections of the same
authoritative store, not a second balance database.

`execute` invokes `atomic` around authentication, all mutations, native movement
and commit. A transactional database host can restore a checkpoint on error.
A host relying on runtime transaction rollback must propagate errors out of the
entry point; swallowing an error and committing would violate the contract.
Physical allocation never grants registration or authority.

The Solana implementation uses runtime signer checks, PDA derivation, program
accounts and SPL calls. `tests/host.rs` implements the same interface using
integer identities, trusted test execution context and transactional memory.
Its tests exercise policy independently and inject settlement/commit failures
after native movement. It is a contract fixture, not a signature algorithm or a
production storage adapter.

## Accounting primitives

The original posting arithmetic and ancestor walk live in `transfer`.
`transfer_debits` enforces same-custodian debit transfers and checks funds before
self-transfer no-ops. These low-level `ledger_lib` primitives assume their caller
has authenticated state and authority. Use `ledger::execute` as the standalone
service boundary. There is no separate kernel crate.

Balances and posting amounts use checked `u128`. Native SPL movement uses `u64`;
wide internal values never truncate into native token transfers.

```bash
cargo test -p cavalre-ledger-core --locked
cargo tree -p cavalre-ledger-core
```
