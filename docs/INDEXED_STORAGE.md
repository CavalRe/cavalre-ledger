# Indexed storage branch

Agreed 2026-10-06. This branch develops the new storage model in
`experiments/indexed-ledger` alongside the existing adapter for comparison.
It supersedes the earlier per-record PDA layout for this implementation only.

## Record

Explicit little-endian encoding, no padding, discriminator, stored relative,
child vector, or sibling links. An AccountRef is a 32-byte container public key
followed by a u32 record index.

| Offset | Field | Bytes |
| ---: | --- | ---: |
| 0 | custodian | 36 |
| 36 | parent | 36 |
| 72 | depth | 1 |
| 73 | kind | 1 |
| 74 | debit | 16 |
| 90 | credit | 16 |
| 106 | child_count | 4 |

Records occupy a contiguous array. `offset(i) = HEADER_LEN + 110 * i`.
Indices are append-only; removal leaves a tombstone and never shifts or reuses
slots. A tombstone has kind 255; other kinds retain the existing encoding.
Child counts allow safe empty-group removal. Clients reconstruct child lists
by reading parent references. Fixed capacity is allocated at container creation
in this initial implementation; growth is not implicit.

## Persistent lookup and metadata

Index namespace: `cavalre.ledger.index`. Conceptual seeds are namespace,
container, parent, relative. The parent AccountRef is split into its container
and little-endian index because a single Solana seed cannot exceed 32 bytes.
Data is exactly four bytes: the little-endian record index.

Metadata namespace: `cavalre.ledger.metadata`. Seeds are namespace, container,
little-endian record index. Data is decimals:u8, name:String, symbol:String,
where each UTF-8 string has a u32 byte length. Metadata is optional and absent
from transfer account lists. There is no accounting-PDA bump in metadata.

Index accounts are program-owned and canonically derived, and registration
creates the index and record atomically. A relative identity cannot authorize
anything without authenticating its signer and binding the lookup to its exact
container and parent. Removing a record closes its lookup; re-registration
receives a fresh record index. Old metadata remains associated with the old
index and is never reassigned.

## Initial executable scope

The first program is an application-owned **internal accounting ledger**.
Its authority policy matches internal ledgers in the existing adapter. It uses
the shared core's `transfer_resolved` walk, including cross-container ancestors,
checked u128 arithmetic and original per-posting Credit/Debit semantics.
Containers belong to one ledger and authority; cross-ledger references fail.
No external-token custody, public debit authorization, wrapping, withdrawals,
or migration from the old adapter is provided by this experimental program.
Those paths remain in the original adapter and must be ported before replacement.
Do not compare its internal transfer CU to an external-token transfer as if
they perform the same admission and backing checks.

Each container header holds the namespace marker, authority, ledger AccountRef
and append count. The ledger's first two records are its root and explicit
Source. Additional containers inherit the ledger and authority from that root.
The program ID is for local simulation only. Nothing is deployed.

## First measurements

Fresh sBPF built with Agave 4.3.0 / platform-tools v1.57, executed by LiteSVM
0.16.0. These are total instruction CU for the unoptimized internal-ledger
prototype, including authorization and posting events. The fixture supplies
both of its 16-record containers, including for the sibling transfer.

| Operation | CU |
| --- | ---: |
| Cross-container transfer, unequal endpoint depths | 21,736 |
| Opposite-polarity transfer | 20,394 |
| Source-to-debit bootstrap | 18,410 |
| Equal-polarity sibling transfer | 9,193 |

This first implementation establishes behavior, not a performance improvement.
It uses the standard Solana entrypoint parser and general shared-core posting
path. A specialized fixed-offset transfer path has not been implemented here.

Reproduce with `bash scripts/check.sh`, then
`cargo test -p cavalre-ledger-runtime-tests --test indexed --locked -- --nocapture`.
The check script builds fresh binaries before runtime tests. The new tests cover
exact bytes, index PDA allocation (including prefunding), optional immutable
metadata, stable tombstones, custody links, cross-container parent walks,
child-count removal guards, overflow, full containers, invalid authorization,
wrong index PDAs, missing containers, cross-ledger rejection and rollback.
