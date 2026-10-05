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
modules are excluded. Solana's program entry points and Anchor SPL movement
dependency are also excluded. The SPL interface types remain for mint/metadata
decoding. The record layout, program identity used for address
validation and shared read code remain available. This is a library build option,
not a command to shut down or replace a deployed program. Stored accounts must
remain available for reads.

## Query behavior

| Query | Behavior |
| --- | --- |
| `account` / Solana `Reader::account_view` | Absolute/relative identities, ledger, effective flags, custody, registration, name, gross balances and parent admission status |
| `name` | Registered name; empty for confirmed unregistered/absent accounts |
| `symbol`, `decimals` | Root asset metadata through the read-only `TokenMetadata` provider; undefined fields return `None` |
| `debit_balance_of`, `credit_balance_of` | Current gross balances with ledger/parent validation |
| `balance_of` | Debit minus credit for debit accounts; credit minus debit for credit accounts, using effective polarity |
| `total_supply` | Root gross debits, as in the original Solidity view; not root net balance |
| `ledger` | Address-only lookup for registered accounts/roots; implicit leaves require their parent context |
| `sub_accounts`, `sub_account` | Registered relative child identifiers; implicit and removed leaves are excluded |
| `ledger_count`, `ledger_at`, `ledgers` | Authoritative global Root count and insertion-ordered entries through `LedgerIndex`; available in Solana `Reader` |

Unsigned negative net balances return an error, preserving Solidity subtraction
behavior. Gross balances remain inspectable. Queries report a parent's admission
restriction without refusing to inspect an implicit leaf. That reported admission
status is not proof of monetary eligibility or spending permission.

## Solana readers

`ledger_view::Reader::from_account_infos` accepts readonly, nonsigner accounts.
It does not invoke the owning program. Off-chain clients can use `Reader::insert`
with account address, owner and bytes returned by RPC. The shared decoder checks
program ownership, header, length and canonical PDA derivation for Ledger records.
It also accepts initialized SPL/Token-2022 mints and canonical Metaplex metadata
accounts. Query resolution requires valid root and custody context where relevant.

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

The global `Root` is the parent and registry of all token and accounting ledgers,
restoring the original `ROOT_ADDRESS` / `subs[ROOT_ADDRESS]` role. Each ledger
remains a debit group at depth 2, with its own credit Source and accounting tree.
The posting walk stops at that ledger; Root has no balance aggregating unlike
assets. Its address is `global_root_address().0`, derived from `["Root"]`.

`Reader::ledger_count()` requires only Root. `ledger_at(index)` additionally
requires the entry at `ledger_index_address(index).0`; `ledgers(start, limit)`
requires only the requested in-range entries. Indices and counts are `u64`.
Results follow registration order, matching Solidity. An out-of-range single
index returns `InvalidIndex`; a page past the end or with zero limit is empty.
Missing in-range entries return `IncompleteIndex`, never a partial success.
No ledger balance records or unrelated index entries are required. Readers
validate owner, header, length and canonical PDA for Root and each entry.

Before the first registration, Root is uninitialized. An omitted or confirmed
absent Root yields `MissingAccount`; clients can separately confirm that this
service has not yet been initialized. `Reader::name` returns `Root` for an
initialized global Root. `Reader::known_ledgers` remains a separate utility
listing only the supplied ledger records, sorted by address.

`LedgerIndex` now provides an authoritative count and indexed lookup rather than
requiring a full list of ledger records. A host must authenticate the unique,
append-only registry at one snapshot. Views remain available with mutations
excluded. See [registration and storage](../crates/cavalre-ledger-solana/README.md#global-root-and-registration).

## Asset metadata

Native SOL roots are identified by `NATIVE_SOL` and report `TokenKind::Native`.
Their gross/net balances and supply are expressed in lamports. `native_symbol()`
returns `SOL` and `native_decimals()` returns `9`; root queries return those same
values without requiring a mint account.

`Reader::symbol(&root)` returns `Result<Option<String>, Error>` and
`Reader::decimals(&root)` returns `Result<Option<u8>, Error>`. Supply the root
record and its mint through `insert` or `from_account_infos`. A decimals query
requires only the mint, whose address must match the root's asset identifier and
whose owner/layout must be the classic SPL or Token-2022 program. Zero decimals
is valid. Decimals describe raw base units; interest/scaled-UI extensions do not
change this value or Ledger's accounting units.

| Symbol source | Validation and selection |
| --- | --- |
| Classic SPL | Canonical Metaplex PDA derived from the root's mint; Metaplex owner, MetadataV1 discriminator and embedded mint must match |
| Token-2022, no metadata pointer | Same canonical Metaplex source |
| Token-2022, pointer to itself | Read the mint's TokenMetadata extension and verify its embedded mint |
| Token-2022, pointer to canonical Metaplex account | Read that authenticated Metaplex account |
| Token-2022, cleared pointer or self-pointer without initialized metadata | `None` |
| Token-2022, pointer to another metadata format | `UnsupportedMetadata`; decimals and balance queries remain available |

The pointer takes precedence over any old inline or Metaplex symbol. Metaplex
strings have trailing NUL padding removed. Symbols are issuer-controlled labels,
not unique token identities. The reader borrows the published metadata prefix
(authority, mint, name, symbol), validates its lengths and UTF-8, and copies only
the returned symbol. It does not fetch URI content or copy unrelated metadata.

For a required Metaplex account, omitted input is `MissingAccount`. Confirmed
absence supplied through `insert_missing` (or an empty System-owned runtime
account) yields `None`. A missing/closed mint cannot establish decimals and is an
error. Wrong owners/addresses and malformed data reject; a malformed inline
symbol yields `InvalidMetadata` without preventing a decimals query. Read all
inputs at one snapshot and construct a new reader after mint/metadata updates.

Accounting-only roots return `None` for both fields because their current record
format defines neither. No symbol is derived from a name and no precision is
invented. Queries accept ledger roots, not descendants; callers that have a leaf
use their known root. This explicitly differs from Solidity's stored per-address
metadata getters. Existing `name` continues to return Ledger's stored name.

Metadata reads add no record fields, token-admission rules or metadata write
instructions. Custom metadata formats and accounting-only
metadata configuration remain outside this implementation.

Sources: [Solana metadata pointers](https://solana.com/docs/tokens/extensions/metadata),
[Metaplex metadata layout](https://github.com/metaplex-foundation/mpl-token-metadata/blob/main/clients/rust/src/generated/accounts/metadata.rs).

Run `bash scripts/check.sh`. In addition to the normal workspace and sBPF tests,
it independently runs both crates with `--no-default-features`, proving that
reads compile and work with the mutation modules excluded.
