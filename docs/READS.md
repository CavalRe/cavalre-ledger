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
extends `ReadStore` and `ChildIndex`; queries never require `Host`, a signature, a fee payer,
token movement or commit. The host may remove mutation dispatch while its query
service continues reading committed snapshots. No pause mechanism is added.

An underbacked external/native ledger also remains readable. Backing is checked
only at the shared mutation boundary, against actual custody and the root's full
credit total. Queries require no vault observation and still expose all claims
while activity is frozen. Direct, uncredited vault top-ups restore mutations
when full backing is restored; partial repairs do not.

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

Ledger addresses are logical identifiers. An external-token root uses the mint
address; native SOL uses `NATIVE_SOL`. Query these identities, not their storage
PDAs. Fetch root records with `root_storage_address(scope, identifier)` and pass
the physical address to `Reader::insert`; the decoder authenticates storage and
the reader indexes the record under its logical identity. Mint metadata and the
Ledger record can coexist in one reader. Accounting-only roots retain their
authority-scoped identity through `ledger_address(authority, identifier)`.

`insert_missing_ledger(scope, identifier)` records confirmed absence of that
root's storage. An existing mint alone says nothing about whether its Ledger
record exists. `insert_missing(address)` remains the helper for confirmed absent
ordinary child records. Every absence observation must come from the same
snapshot as the supplied records.

`ledger_lib::name_to_address(name)` derives a relative identity;
`to_address_by_name(program, parent, name)` derives its absolute child PDA and
bump. Both are available without mutations and require no account reads. These
helpers derive identities, not search by display label. An explicitly identified
account may have the same label at another address. The exported `SOURCE`
identifier equals `name_to_address("Source")`; both resolve the reserved credit
account when combined with the ledger root.

| Query | Behavior |
| --- | --- |
| `account` / Solana `Reader::account_view` | Absolute/relative identities, ledger, effective flags, custody, registration, name, gross balances and parent admission status |
| `name` | Registered name, which may be empty for a leaf; also empty for confirmed unregistered/absent accounts. Use `account_view.registered` to determine registration. |
| `symbol`, `decimals` | Stored values at the queried address; confirmed absence returns empty symbol and zero decimals. Ledger metadata remains the registration snapshot, without reading the mint. |
| `debit_balance_of`, `credit_balance_of` | Current gross balances with ledger/parent validation |
| `balance_of` | Debit minus credit for debit accounts; credit minus debit for credit accounts, using effective polarity |
| `total_supply` | Stored gross debits at the queried address, as in Solidity; confirmed absence returns zero. At a token ledger this is its total supply, not its net balance. |
| `ledger` | Address-only lookup for registered accounts/roots; implicit leaves require their parent context |
| `sub_account_count`, `sub_accounts`, `sub_account` | Stored child count and registered child identifiers; Root children are ledger addresses, other children are relative identifiers |
| `sub_account_index` | One-based position in the parent's child array; zero for a confirmed unregistered account |
| `ledger_count`, `ledger_at`, `ledgers` | Thin wrappers over Root's child count, child lookup and child pagination |

Unsigned negative net balances return an error, preserving Solidity subtraction
behavior. Gross balances remain inspectable. Queries report a parent's admission
restriction without refusing to inspect an implicit leaf. That reported admission
status is not proof of monetary eligibility or spending permission.

`total_supply`, `symbol` and `decimals` do not require the queried address to be
a registered ledger root. They read its own stored fields, including for groups
and allocated implicit leaves; metadata is not inherited from a parent. They do
not prove registration or ledger kind. The existing Rust metadata return types
are retained: defaults are `Some("")` and `Some(0)`. Unknown input and storage
validation errors still propagate, rather than becoming defaults.

## Transfer write planning

Custody operations (`wrap`, `unwrap`, `wrap_sol`, `unwrap_sol`) also accept a
read-only token ledger root for zero amounts. Every Ledger record can be
read-only in that case; vault/wallet and payer remain writable for the unchanged
native transfer path. For nonzero amounts, explicitly set the root writable in
the client account metas, along with the Source, endpoint and changed ancestors.
The generated `MoveTokens`/`MoveSol` account metas do not set this automatically.
Clients and application programs can use `ledger_cpi::set_custody_root_writable`
with the crate's `cpi` feature to adjust an encoded instruction in place. It reads
the amount without re-encoding data and retains all other account privileges;
the outer transaction must provide all required writes. See the
[CPI example](../crates/cavalre-ledger-solana/README.md#instructions).

`ledger_view::transfer_writable_accounts` in the core and the matching Solana
`Reader` method reuse the original posting walk to return the Ledger records a
transfer changes. Both work with mutations disabled. The inputs are the ledger,
two `(parent, relative)` endpoints and the amount; effective flags determine
polarity, including inheritance for unregistered leaves.

The core returns logical record identities. The Solana `Reader` translates a
changed root to its physical storage PDA before returning the write set, so its
output can be applied directly to transaction account metas. This never makes
the token mint writable. Instruction arguments such as `parent` still use the
logical ledger address.

For same-polarity transfers, both paths stop below their lowest common ancestor.
That ancestor and everything above it remain read-only. For opposite-polarity
postings, both gross columns change through the token ledger root. Records
already allocated but unchanged need no write access. Absent endpoints are
included only when a posting changes their balance and therefore needs storage.
Zero-amount and self-transfers return an empty Ledger write set even when the
endpoints have no storage. Authorization, admission and applicable balance
checks still run; zero-amount posting events are preserved. Storage allocation
does not register a leaf.

```rust,ignore
use cavalre_ledger_solana::ledger_lib::Child;

let writable = reader.transfer_writable_accounts(
    &root,
    Child { parent: from_parent, relative: from },
    Child { parent: to_parent, relative: to },
    amount,
)?;
// Apply only to Ledger record metas; retain payer and signer requirements.
for meta in &mut ledger_record_metas {
    meta.is_writable = writable.contains(&meta.pubkey);
}
```

Supply the endpoints, parents, custody ancestors and posting paths from one
consistent snapshot. Insert confirmed missing endpoints explicitly; omitted
records are errors, never assumed absent. Continue passing unchanged records
needed for authentication or inspection as read-only accounts. This helper
plans Ledger record writes only: it does not authorize a transfer, select
settlement accounts, or make a stale transaction valid. Runtime execution
recomputes the walk from authenticated state. A missing required writable
account fails atomically; refresh and rebuild if the tree changed.

External/native transfers and tree changes additionally require their canonical
vault `["vault", root_storage]` as a read-only remaining account. Append it to
the instruction after planning Ledger record writes; it is custody input, not a
Ledger record for the Reader snapshot. Its current backing is checked at execution
even for zero/self/repeated operations. Accounting-only calls need no vault.

Account privileges are chosen before signing. Across multiple instructions,
use the union of required writes. A CPI caller must supply those same privileges
in the outer transaction. Payers, token wallets, vaults and other application
state have their own write requirements. See `tests/runtime/writable_accounts.rs`
for direct and CPI examples with unchanged ancestors supplied read-only.

## Solana readers

`ledger_view::Reader::from_account_infos` accepts readonly, nonsigner accounts.
It does not invoke the owning program. Off-chain clients can use `Reader::insert`
with account address, owner and bytes returned by RPC. The shared decoder checks
program ownership, header, length, canonical PDA derivation and the stored bump
for Ledger records.
It also accepts initialized SPL/Token-2022 mints and canonical Metaplex metadata
accounts. Query resolution requires valid root and custody context where relevant.

`ledger_lib::decode(AccountInfo)` uses the runtime-authenticated owner and the
canonical bump stored by Ledger at initialization to verify an address in one
derivation. The runtime does not accept a caller-provided bump in its place.
`decode_data` and Reader validate raw snapshots by deriving the canonical bump
again and comparing it with the stored value. Both paths reject a stored bump
that does not match its account address.

For a confirmed RPC `null`, call `insert_missing`. A runtime reader can provide
an empty System-owned account for an absent PDA. Omitting an account does not
prove absence: querying unknown input returns `MissingAccount`, never a guessed
zero. An allocated implicit leaf retains its actual balances while its effective
flags come from its registered parent.

For account and balance queries, supply the root, parent, custody ancestor and
endpoint. Read one consistent snapshot; RPC provenance and commitment are the
client's responsibility. Duplicate entries are rejected.

For stored-value getters on an allocated non-root record, supply the record and
its token ledger root so the existing reader can authenticate ledger membership.
Parents and custody ancestors are unnecessary for these getters. A confirmed
absent address needs no root context to return its defaults.

Child enumeration uses a maintained index, matching Solidity's `subs[parent]`
and `subIndex[child]`. Creation appends; removal moves the last child into the
removed position and updates its reverse index. Implicit leaves are excluded.
No account scan or sorting occurs. Positions are zero-based for reads; stored
`sub_index` is one-based, with zero reserved for unregistered accounts.

On Solana, `child_index_address(&parent, index)` derives the ordinary child slot
PDA from `["subs", parent, index.to_le_bytes()]`. Each 80-byte slot authenticates
its parent and position and stores the relative identity. Root slots instead
store the ledger's absolute address, as in Solidity. `Reader::insert` and
`from_account_infos` validate slot ownership, discriminator, length and PDA.

For `sub_accounts(ledger, parent, start, limit)`, provide the usual ledger/parent
context and **only the requested slots**. Child records and other sibling slots
are unnecessary. A missing requested slot returns `IncompleteIndex`; a cleared
slot within the recorded count is `InvalidIndex`. A zero limit or a page beyond
the end returns an empty page without requiring any slots. `sub_account` loads
one slot and rejects positions outside the stored count.

Ledger discovery uses this same index on global `Root`. There is no separate
ledger registry or ledger-specific indexing mechanism. `global_root_address().0`
derives Root from `["Root"]`.

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

The count is Root's stored `u32` child count and requires only Root. To fetch
`ledgers(100, 10)`, read Root and slots 100 through 109 (clipped to its count),
then insert those bytes into a Reader. No ledger records or other slots are
required. `ledger_at(i)` requires only Root and slot `i`.

Use a consistent snapshot for counts and slots. Like Solidity swap-and-pop,
removal can change a position; positions are not permanent account identities.

`Reader::known_ledgers` remains a separate partial-snapshot utility: it lists
only supplied ledger records and does not claim to discover all Root children.
Views and Root decoding remain available with mutations excluded. See
[Root creation](../crates/cavalre-ledger-solana/README.md#global-root).

## Asset metadata

Ledger stores `name`, `symbol` and `decimals` together at registration, following
Solidity `addLedger` and `addExternalToken`. Name and symbol must contain 1–64
UTF-8 bytes. Zero decimals is valid. Queries use only the ledger record; later
issuer metadata changes do not alter the stored values. `symbol` and `decimals`
retain their existing optional Rust return types for compatibility, but a valid
initialized ledger has both values, including accounting-only ledgers.

`add_ledger(id, name, symbol, decimals)` accepts explicit metadata from the
accounting ledger's authenticated authority. Native SOL stores `SOL`, `SOL`, `9`.
`add_external_token()` accepts no caller-supplied metadata. It reads mint decimals
and authenticated issuer metadata using the following source rules:

| Token metadata source | Registration behavior |
| --- | --- |
| Classic SPL | Requires the canonical Metaplex metadata PDA for the mint |
| Token-2022 without a metadata pointer | Requires canonical Metaplex metadata |
| Token-2022 pointing to itself | Requires inline TokenMetadata with matching mint |
| Token-2022 pointing to canonical Metaplex | Requires that account; inline metadata does not override the pointer |
| Cleared pointer or missing inline metadata | Rejects registration |
| Another metadata format | Rejects as unsupported |

Supply required Metaplex metadata as a readonly remaining account. Its owner,
PDA, discriminator, embedded mint, UTF-8 and string lengths are validated.
Metaplex's own limits of 32 name bytes and 10 symbol bytes apply; trailing NUL
padding is removed. Inline Token-2022 fields use Ledger's 64-byte limits. Empty,
missing, forged or malformed required metadata cannot create a ledger, Source,
child index entry or vault. All initialization effects roll back on rejection.
URI contents and unrelated optional metadata fields are not fetched.

`Reader::external_metadata(mint)` exposes the authenticated issuer fields for
pre-registration reads; it requires the mint and selected metadata source.
This is distinct from stored ledger metadata. Ledger initialization applies the
name/symbol validity rules to those fields and saves the snapshot.

Matching repeated registration is a successful no-op, with no new allocation,
child index changes or creation events. Conflicting name, symbol or decimals
reject. External registration re-reads issuer metadata on each request, as the
original Solidity does: if an issuer changes it after initial registration,
registration rejects the conflict while existing Ledger reads remain unchanged.

Sources: [Solana metadata pointers](https://solana.com/docs/tokens/extensions/metadata),
[Metaplex metadata layout](https://github.com/metaplex-foundation/mpl-token-metadata/blob/main/clients/rust/src/generated/accounts/metadata.rs).

Run `bash scripts/check.sh`. In addition to the normal workspace and sBPF tests,
it independently runs both crates with `--no-default-features`, proving that
reads compile and work with the mutation modules excluded.
