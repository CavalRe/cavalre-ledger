# Ledger events

The shared service emits the original Ledger event families through `Host::emit`.
The core decides which events occur and in what order; each host owns encoding
and delivery. Solana uses Anchor `#[event]` types and `emit!` program logs.
No event queue, second accounting store or additional account layout is introduced.

## Event contract

| Event | Solana fields | Emission rule |
| --- | --- | --- |
| `LedgerAdded` | `ledger`, `scope`, `identifier`, `name`, `symbol`, `decimals` | Once after creating the root and its Source |
| `SubAccountAdded` | `ledger`, `parent`, `relative`, `is_credit` | A leaf becomes registered, including Source creation |
| `SubAccountGroupAdded` | `ledger`, `parent`, `relative`, `name`, `is_credit` | A group becomes registered |
| `SubAccountRemoved` | `ledger`, `parent`, `relative` | A registered empty leaf is removed |
| `SubAccountGroupRemoved` | `ledger`, `parent`, `relative` | A registered empty group is removed |
| `Credit` | `ledger`, `account`, `amount`, `balance` | Each credit posting in the original ancestor walk |
| `Debit` | `ledger`, `account`, `amount`, `balance` | Each debit posting in the original ancestor walk |

`ledger`, `parent` and posting `account` are absolute addresses. Structural
`relative` identifies a child under its absolute parent; derive the child's PDA
using both. All monetary values are `u128` raw base units.

Initialization emits `SubAccountAdded` for the protected credit Source before
`LedgerAdded`, matching the original order. Repeated matching ledger, leaf or group
registration emits nothing. Removal of an unregistered target emits nothing.
Funding an implicit leaf emits posting events without claiming it was registered;
later explicit registration emits the corresponding structural event.

Posting events follow the original depth-aligned walk: the credit side precedes
the debit side at a shared depth. Same-polarity paths stop below their shared
ancestor, so no canceled ancestor events are emitted. Opposite-polarity paths
continue through shared groups and the root, reporting both gross columns.
The write buffer records each posting direction alongside its existing balance
change, including zero-amount postings; it does not infer events from net deltas.

`Credit` and `Debit` describe the accounting operation, not the gross balance
column. A credit reduces a debit leaf's debit balance or increases a credit
leaf's credit balance. Its original leaf polarity determines the column all the
way up the tree, regardless of ancestor group classifications. `balance` is that
column's resulting gross balance, never the net balance. Self-transfers emit no
Ledger debit/credit events, after the required authorization and balance checks.
Zero-amount transfers between distinct endpoints still emit their posting events.

## Explicit compatibility decisions

This is a Solana encoding of the original semantics, not the Solidity event ABI.
Public keys replace EVM addresses and `u128` replaces `uint256`. Solana logs do
not supply Solidity indexed topics. The event discriminator identifies the type.

Solana separates the ledger PDA from the asset identity. `LedgerAdded` therefore
includes `ledger`, `identifier` and `scope`: external mints and native SOL use
zero scope; accounting-only roots use their owning authority. The identifier is
the mint, `NATIVE_SOL`, or the application's accounting quantity identifier.
`name`, `symbol` and `decimals` are the stored registration snapshot, preserving
the original Solidity creation metadata. External-token values come from the
validated issuer source; accounting-only values come from the authorized caller.
Native SOL stores `SOL`, `SOL`, `9`. Matching repeat registration emits no event.

`SubAccountGroupAdded` also includes the relative identifier. It cannot be
reconstructed from the name when an application supplies an independent identity.
The original leaf-add and removal payloads retain their relative-identity meaning.

These specific events replace the draft `AccountChanged` snapshots. Consumers
of that draft schema must update. ERC20 wrapper `Transfer` and `Approval` events
are not emitted: this service has no ERC20 wrapper or allowance interface.
Native SOL, classic SPL and supported Token-2022 settlement all use the same
Ledger posting event rules. Their underlying token/System instructions remain
separately observable in the transaction.

## Transaction status and delivery

Solana retains execution logs even when an instruction or a later instruction
in the same transaction fails. **Only index events from successful transactions**
at the consumer's chosen commitment. Inspect the entire transaction's `meta.err`,
not just a Ledger instruction's success message. Events emitted before failed
allocation, commit, or a later transaction failure are speculative; all Ledger
and token state changes roll back. Hosts with buffered event delivery must
discard their event buffer on transaction failure.

Attribute program-data logs using the invocation stack: a consumer CPI or another
program in the transaction may emit its own data. Decode only data emitted by
Ledger. Deduplicate indexed events by transaction signature and log position.

`emit!` uses program logs, which runtime/provider limits can truncate. This is
not a guaranteed complete archival journal. Account state remains authoritative;
clients requiring full history need a transaction stream and storage strategy
that preserves the required logs. `emit_cpi!` is not enabled in this revision;
it adds CPI compute and accounts and requires a separate delivery decision.
Applications still budget the complete transaction, including event emission.

The core tests cover event order, gross-column meaning, cancellation, zero and
self transfers, structural no-ops and rollback. Runtime tests decode actual sBPF
logs for initialization, lifecycle, direct/CPI settlement and failed transactions.
The standard execution profile includes the cost of these events.

Sources: [original events](https://github.com/CavalRe/cavalre-contracts/blob/34d159ff4e88fdfdee16738d9a1228f0bf407212/modules/ledger/ILedger.sol),
[original emission rules](https://github.com/CavalRe/cavalre-contracts/blob/34d159ff4e88fdfdee16738d9a1228f0bf407212/modules/ledger/LedgerLib.sol),
[Anchor events](https://www.anchor-lang.com/docs/features/events),
[Solana log notifications](https://solana.com/docs/rpc/websocket/logssubscribe).
