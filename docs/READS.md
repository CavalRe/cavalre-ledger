# Reads survive removal of mutations

The original Solidity split had an operational purpose: removing `Ledger.sol`
from dispatch disabled its mutations while `LedgerView.sol` continued reading
the same storage. The Rust implementation preserves that boundary.

| Module | Responsibility |
| --- | --- |
| `ledger.rs` | Authenticated mutations and atomic settlement through a host |
| `ledger_view.rs` | Non-mutating queries over validated state |
| `ledger_lib.rs` | Shared identities, account types, record format and accounting helpers |

The core view uses `ReadStore` and address derivation. The mutating `Host`
extends `ReadStore`; queries never require `Host`, a signature, a fee payer,
token movement or commit. The host may remove mutation dispatch while its query
service continues reading committed snapshots. No pause mechanism is added.

`ReadStore::account` borrows names from host storage through `Account<A, &str>`.
Flags, custody, gross/net balance and supply queries allocate no account metadata.
Names are copied only when a query explicitly returns an owned name or a full
`AccountView`. The Solana `Reader` still decodes supplied account bytes once;
borrowed queries reuse those validated records.

Both crates default to enabling `mutations`. A read client can opt out:

```toml
cavalre-ledger-core = { path = "../crates/cavalre-ledger-core", default-features = false }
cavalre-ledger-solana = { path = "../crates/cavalre-ledger-solana", default-features = false }
```

Adjust paths to the consuming project. With defaults disabled, the `ledger`
modules are excluded. Solana's program entry points and SPL movement dependency
are also excluded. The record layout, program identity used for address
validation and shared read code remain available. This is a library build option,
not a command to shut down or replace a deployed program. Stored accounts must
remain available for reads.

## Query behavior

| Query | Behavior |
| --- | --- |
| `account` / Solana `Reader::account_view` | Absolute/relative identities, ledger, effective flags, custody, registration, name, gross balances and parent admission status |
| `name` | Registered name; empty for confirmed unregistered/absent accounts |
| `debit_balance_of`, `credit_balance_of` | Current gross balances with ledger/parent validation |
| `balance_of` | Debit minus credit for debit accounts; credit minus debit for credit accounts, using effective polarity |
| `total_supply` | Root gross debits, as in the original Solidity view; not root net balance |
| `ledger` | Address-only lookup for registered accounts/roots; implicit leaves require their parent context |
| `sub_accounts`, `sub_account` | Registered relative child identifiers; implicit and removed leaves are excluded |
| `ledger_count`, `ledger_at`, `ledgers` | Core queries requiring a complete host `LedgerIndex` |

Unsigned negative net balances return an error, preserving Solidity subtraction
behavior. Gross balances remain inspectable. Queries report a parent's admission
restriction without refusing to inspect an implicit leaf. That reported admission
status is not proof of monetary eligibility or spending permission.

## Solana readers

`ledger_view::Reader::from_account_infos` accepts readonly, nonsigner accounts.
It does not invoke the owning program. Off-chain clients can use `Reader::insert`
with account address, owner and bytes returned by RPC. The shared decoder checks
program ownership, header, length and canonical PDA derivation. Query resolution
also requires valid root and custody context.

For a confirmed RPC `null`, call `insert_missing`. A runtime reader can provide
an empty System-owned account for an absent PDA. Omitting an account does not
prove absence: querying unknown input returns `MissingAccount`, never a guessed
zero. An allocated implicit leaf retains its actual balances while its effective
flags come from its registered parent.

Supply the root, parent, custody ancestor, endpoint and any children needed by
the query. Read one consistent snapshot; RPC provenance and commitment are the
client's responsibility. Duplicate entries are rejected. For registered-child
enumeration, the supplied records must match the parent's stored child count;
otherwise the query returns `IncompleteIndex`. Child order is absolute-PDA order
within the snapshot, rather than Solidity's insertion/swap-removal order.

`Reader::known_ledgers` lists only supplied roots. It does not claim to be a
global ledger count. A complete Solana discovery client must obtain the full
root set, for example from program-account queries, before exposing the core
`LedgerIndex` contract. No global on-chain registry or second balance store is
introduced by this change.

## Compatibility limits

The current stored format includes names but not token symbols or presentation
decimals. External decimals remain in the mint; token symbols require their
metadata source, and accounting-only precision remains application policy.
The original `symbol`, `decimals` and native-asset metadata views are not falsely
implemented with invented defaults. This change adds no new metadata fields and
does not change the stored account layout or public mutation instructions.

Run `bash scripts/check.sh`. In addition to the normal workspace and sBPF tests,
it independently runs both crates with `--no-default-features`, proving that
reads compile and work with the mutation modules excluded.
