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

## Logical identities and physical storage

`to_address(parent, relative)` is exactly `Keccak256(parent32 || relative32)`.
It returns one logical address, with no program ID, prefix, bump or curve test.
`name_to_address(name)` hashes the name; `to_address_by_name(parent, name)` then
uses the same child hash. `SOURCE` is the precomputed hash of `"Source"`.
Relative addresses retain their authorization role. Knowing an absolute hash
does not grant authority; the verified signer must satisfy the original custody
ancestry rules.

External roots use the mint address; native SOL uses `NATIVE_SOL`. Their physical
container is `root_storage_address(scope, identifier)`. Accounting-only roots
retain their authority-scoped root identity. Global Root is the existing fixed
constant. These root conventions are distinct from child hashing.

Every registered group has a physical container. Leaves live in a mapping in
their parent's container; there are no separate leaf PDAs or child-index PDAs.
`account_storage_address(parent, relative)` derives a **group container**, not
the logical child identity. Instruction arguments, parent links, events and query
results use logical addresses. Transaction account metas use physical containers.

`ledger_lib::decode(AccountInfo)` authenticates the owner, container layout and
physical address using the stored bump and hash-only SDK derivation. Creation
still derives an off-curve physical PDA for allocation. Raw RPC snapshots use
`decode_data`, which also checks canonical bump derivation. No child identity
uses PDA derivation. Existing owned records do not repeat curve validation.

## Query behavior

Supply a consistent snapshot of the root, relevant groups and custody ancestors
to `Reader::insert` or `Reader::from_account_infos`. The reader indexes group
headers and their mapped leaves by logical identity. Omission of a required
group container is an error. An authenticated parent container proves whether
a particular leaf exists; clients do not fetch absent leaf PDAs.

`record_at(container, logical)` reads a selected header or mapped leaf without
constructing a whole Reader. A group reference requires that group's container.
Mutation execution also searches only requested mapping entries. The convenience
Reader decodes supplied containers into its snapshot indexes; it is not a
constant-memory reader for arbitrarily large containers.

For a confirmed RPC null root, use `insert_missing_ledger(scope, identifier)`.
An existing mint alone does not prove that its Ledger container exists.
`insert_missing(address)` remains available for explicit snapshot absence.
Never turn an omitted or unvalidated group into an absent leaf observation.

| Query | Behavior |
| --- | --- |
| `account_view` | Logical identities, effective flags, custody, registration, labels, gross balances and parent admission |
| `name` | Registered label, including empty leaf labels; otherwise empty |
| `symbol`, `decimals` | Own stored metadata; confirmed absence gives empty symbol and zero decimals |
| `debit_balance_of`, `credit_balance_of` | Gross balances with ledger and parent validation |
| `balance_of` | Net balance using effective debit/credit polarity; unsigned underflow rejects |
| `total_supply` | Own gross debit value; confirmed absence returns zero |
| `sub_account_count`, `sub_account`, `sub_accounts` | Registered child count and maintained insertion/swap-pop order |
| `sub_account_index` | One-based child position; zero for unregistered leaves |
| `ledger_count`, `ledger_at`, `ledgers` | The same queries over global Root's children |

Metadata getters do not require the queried address to be a registered ledger;
metadata is not inherited. Existing optional Rust result types remain. Unknown
input and invalid storage still return errors. An implicit leaf keeps its stored
balance while inheriting flags from its registered parent. A restrictive parent's
policy is reported by views without hiding balances.

## Transfer write planning

The core's `transfer_writable_accounts` returns changed **logical** records using
the original posting walk. The Solana Reader maps these to physical containers,
then deduplicates them. Set writable account metas from that result. Authentication
inputs whose containers are unchanged remain readonly. This never marks the mint
itself writable.

Same-polarity posting cancels below the lowest common ancestor: its logical
balance is unchanged. However, a parent container holding either changed leaf
must be writable even when the parent's own balance does not change. Two
transfers among siblings therefore contend on that container. Transfers in
separate subgroups can retain independent containers. This is a physical locking
regression from one physical record per leaf; the accounting cancellation remains.

Zero/self transfers need no Ledger writes or allocation. Authorization, admission,
backing and required funds checks still run. First receipt creates a mapping entry
without registering the leaf or requiring the receiver to sign.

```rust,ignore
let writable = reader.transfer_writable_accounts(&root, from, to, amount)?;
for meta in &mut ledger_container_metas {
    meta.is_writable = writable.contains(&meta.pubkey);
}
```

Supply root, endpoint parent containers, any group endpoints, custody ancestors
and posting paths once each. All account privileges are selected before signing;
CPI needs the same privileges in the outer transaction. External/native calls
also require their canonical custody account readonly for the backing check.
It is an input to custody validation, not a Ledger container.

Nonzero wrap/unwrap also writes the root container, which contains Source.
`ledger_cpi::set_custody_root_writable` still adjusts generated instructions for
this requirement. Zero custody amounts leave Ledger containers readonly; payer,
token wallet and custody declarations retain their separate write requirements.
Missing write access fails atomically, including native token movement.

## Mapping layout and child enumeration

A container begins with the existing 512-byte group header, followed by a
16-byte mapping header. External/native roots retain the authenticated custody
address at bytes 480–511; balance and metadata writes preserve it. Custody balances
are read fresh, never cached. The base container allocation is now 528 bytes.

Each mapping capacity slot costs 288 bytes: a 32-byte logical key, 224 bytes
for a compact leaf or group reference, and 32 bytes for child-vector capacity.
Leaves inherit redundant root/parent/custodian/depth fields from the authenticated
container. Group references point to the authoritative group header; they do not
hold a second copy of its balance. Capacity grows exactly as required and is
retained on removal. Rent reclamation is not implemented.

Mapping entries are sorted by logical key for binary lookup. Insertion may move
existing bytes; a balance-only update changes only the target fields. The
separate child vector preserves insertion order and swap-and-pop semantics.
Enumeration does **not** sort accounts or scan the program's entire account set.
Implicit leaves are excluded from this registered child vector. Root entries
contain absolute ledger addresses; other groups enumerate relative identities.

Read a parent container to enumerate its children; child containers are unnecessary
for listing. Root discovery likewise needs only Root. RPC currently fetches the
whole container, not isolated child slots. Removing a registered group may also
need the swapped last child's group container to update its reverse index.
Repeated removal of an unregistered child needs no unrelated sibling container.

The mapping is not paged. Solana's physical account-size limit applies per group,
and growth/insertion cost increases with occupied capacity. This is an explicit
storage/scaling limitation of this draft, not a new logical tree-depth rule.
See [transfer costs and regressions](TRANSFER_COSTS.md) before selecting a layout
for a large application.

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
