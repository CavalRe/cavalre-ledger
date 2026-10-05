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

## Independent read interface

`ledger_view.rs` corresponds to `LedgerView.sol`. It depends only on the shared
`ReadStore` and address derivation in `ledger_lib.rs`, with optional child/root
indexes for enumeration. It has no dependency on `ledger.rs`, authentication,
token settlement or commit. The mutation `Host` extends that read interface.

Read queries resolve names, effective account flags, custody, gross/net balances,
total supply and registered children. They validate identities and parent/ledger
membership but do not enforce mutation permission or monetary admission. A
restricted implicit leaf remains readable. `None` from `ReadStore::account`
means confirmed absence; unavailable state must return an error.

`symbol` and `decimals` validate a ledger root and read through the optional
`TokenMetadata` trait. The host authenticates each metadata source and returns
`None` for undefined fields, errors for missing/invalid input, and the actual
base-unit precision (including zero). This adds no platform types, stored fields
or metadata requirement to accounting-only hosts.

`ReadStore::account` returns `Account<A, &str>` with a borrowed name. Flags,
custody and balance reads use this borrowed projection and allocate nothing.
`Account<A>` still defaults to an owned `String` for creation and metadata
changes. Views that explicitly return names copy them into their result; numeric
queries do not construct a named account result.

Build with `default-features = false` to exclude `ledger.rs`. Shared account
types, LedgerLib primitives and LedgerView stay available. This supports removing
the mutation module while keeping queries connected to the same stored state;
it does not add a pause flag. See [query semantics and host integration](../../docs/READS.md).

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
| `account`, `put` | Borrow authenticated logical state; create or replace metadata without imposing a serialization or database format. |
| `set_balances`, `set_children` | Update existing fields directly, preserving all unrelated metadata. Missing accounts must error. |
| `token_balances`, `move_tokens` | Validate supported native assets, bind the vault and wallet to this operation, observe balances and execute native movement. |
| `emit` | Encode/deliver semantic Ledger events within the transaction; discard failed events or expose transaction status with speculative logs. |
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

`execute` invokes `atomic` around authentication, all mutations, events, native movement
and commit. A transactional database host can restore a checkpoint on error.
A host relying on runtime transaction rollback must propagate errors out of the
entry point; swallowing an error and committing would violate the contract.
Physical allocation never grants registration or authority.

The host contract does not require full before/after copies. The Solana host
decodes each supplied record once, reserves its record buffer once, and tracks
which entries changed. It uses runtime rollback and writes only changed records.
The posting walk reserves one bounded change buffer for the two ancestor paths;
its arithmetic, cancellation and validation order are unchanged.
The same change buffer records credit/debit posting directions and gross columns
for event emission, including zero amounts. No second event buffer or ancestor
walk is needed. See [event semantics](../../docs/EVENTS.md).

Host implementors must adapt the borrowed `account` return type and implement
`set_balances`/`set_children` inside the same atomic boundary. This is a Rust
interface change; it changes neither Solana instruction arguments nor stored
account encoding.

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
