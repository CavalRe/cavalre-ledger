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
| `sub_account_count`, `sub_accounts`, `sub_account` | Stored child count and registered child identifiers; Root children are ledger addresses, other children are relative identifiers |
| `ledger_count`, `ledger_at`, `ledgers` | Thin wrappers over Root's child count, child lookup and child pagination |

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

Ledger discovery is ordinary child enumeration on global `Root`. Root uses the
same account record and `children` field as every other group. A ledger's
`parent` points to Root; there is no separate ledger list, `LedgerIndex` provider
or index-entry account. `global_root_address().0` derives Root from `["Root"]`.

| Discovery query | Shared query |
| --- | --- |
| `Reader::ledger_count()` | `sub_account_count(Root, Root)` |
| `Reader::ledger_at(i)` | `sub_account(Root, Root, i)` |
| `Reader::ledgers(start, limit)` | `sub_accounts(Root, Root, start, limit)` |

Root is a registered group at depth 1. Its children are the debit ledger groups
at depth 2, each with its own credit Source and accounting tree. Root itself
has no token balances; posting still stops at the selected ledger. Root's name
is read through the ordinary `name` query. Root is initialized when its first
ledger is created; an unknown Root is `MissingAccount`.

The count is Root's stored `u32` child count and requires no child records.
Indexed lookup and pagination have the existing child-reader semantics: supply
Root and all its registered immediate children at a consistent snapshot. They
are sorted by absolute address, and their count must match Root's `children`;
missing records return `IncompleteIndex`. Token Sources and deeper descendants
are not needed. On a complete snapshot, out-of-range lookup is `InvalidIndex`,
a page beyond the end is empty, and a zero limit returns an empty page.
Pagination slices that complete snapshot; it does not fetch missing records.
This is the same order/completeness behavior as other Solana child enumeration,
and differs from Solidity's insertion/swap-removal order.

`Reader::known_ledgers` remains a separate partial-snapshot utility: it lists
only supplied ledger records and does not claim to discover all Root children.
Views and Root decoding remain available with mutations excluded. See
[Root creation](../crates/cavalre-ledger-solana/README.md#global-root).

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
