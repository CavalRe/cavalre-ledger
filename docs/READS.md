# Account and metadata storage

All accounting addresses are the PDAs of their actual storage accounts. `parent`
and `custodian` contain those addresses. An external mint is asset configuration;
it is not substituted for its ledger's storage address.

## Accounting record

The PDA seeds are `cavalre.ledger.account`, parent PDA, relative identity, and the
canonical bump. The relative identity is supplied by the caller, may be any
32-byte value, and is kept in the parent's children vector. It is not repeated
in the child. Ownership and instruction identity checks are separate from
whether a relative identity is on the Ed25519 curve.

All integers are little-endian. There is no alignment padding.

| Offset | Field | Bytes |
| --- | --- | --- |
| 0 | parent PDA | 32 |
| 32 | custodian PDA | 32 |
| 64 | kind: debit group, credit group, debit leaf, credit leaf | 1 |
| 65 | depth | 1 |
| 66 | debit balance (`u128`) | 16 |
| 82 | credit balance (`u128`) | 16 |
| 98 | child_index (`u32`, one-based) | 4 |
| 102 | ordinary account's children vector | 4-byte length, then 32 bytes per child |

A leaf is **106 bytes**. Groups append their relative child identities. The vector
length is the only child count. Removal swaps in the last relative identity,
updates the moved child's index, and pops the vector. A removed, empty account
is closed; receiving a first balance creates and indexes an account atomically.
Removal requires its metadata PDA, even if empty, so existing metadata is closed
in the same operation.
There is no persisted unregistered state or per-group registration policy.

A ledger at depth 2 adds fixed configuration before its vector:

| Offset | Field | Bytes |
| --- | --- | --- |
| 102 | token kind: native, external, internal | 1 |
| 103 | asset mint or internal identifier | 32 |
| 135 | internal-ledger authority; zero for external/native | 32 |
| 167 | custody vault; zero for internal | 32 |
| 199 | children vector | 4-byte length, then 32 bytes per child |

A ledger starts at 203 bytes before its children. Its mandatory Source makes
its initial accounting size 235 bytes. Internal ledger relative identities are
Keccak-256(authority || identifier), preserving authority scoping; external
ledger relatives are mint addresses, and native SOL uses the zero identifier.
All ledger records are children of global Root. Root retains its distinguished
`Root` PDA and depth-1 header, with zero balances and child_index zero.

There is no per-account ledger pointer, relative field, format, discriminator,
bump, cached authority, metadata string, decimals, or separate child-index PDA.
For descendants, the ledger is the custodian record's parent.

## Metadata record

The independent PDA seeds are `cavalre.ledger.metadata`, the same parent and
relative identity, and this metadata PDA's own canonical bump.

| Field | Encoding |
| --- | --- |
| bump | 1 byte: associated accounting PDA's bump |
| decimals | 1 byte: ledger precision; unused for ordinary descendants |
| name | u32 byte length followed by UTF-8 |
| symbol | u32 byte length followed by UTF-8 |

Metadata takes 10 bytes plus the strings. The existing 64-byte limits on each
string remain. A metadata bump and an accounting bump are independently derived;
the stored `bump` is specifically the accounting bump. Metadata has no format or
kind field. A future layout uses a new namespace and therefore new PDAs.

Metadata is optional. Supply its PDA on creation to persist labels. An authorized
caller can attach the first label to an existing unlabeled account through the
same add instruction, without changing its balance or child index. Existing labels
remain immutable. External ledger metadata snapshots the authenticated mint's
metadata at creation; decimals come from the mint. Descendant unit queries use
the ledger's metadata. Transfers do not take metadata accounts.

## Reads and authentication

RPC clients can request a byte slice for a fixed field. On-chain code can borrow
and read/write the fixed fields directly. The runtime still loads the account's
whole data; avoiding decoding does not eliminate that loading cost.

The mutation adapter reads a `Header` containing fixed fields and the child
count. It reads an existing child directly at its stored index and stages only
changed slots. Commit resizes the account, writes the header, and appends or
overwrites those slots in place; it never copies untouched siblings. Removal
swaps the last slot into the removed position and shrinks the trailing array.
The full `Record` decoder remains available for clients that enumerate children.

The namespace is implicit in the authenticated PDA, not extractable from the
address bytes. Structural decoding alone does not authenticate an arbitrary
program-owned account's type. Use an expected PDA or a typed pointer already
established by the program. `decode_data` checks owner/layout and additionally
checks the configured identity for global Root and ledgers. `Reader` binds child
queries to their parent's vector and the accounting PDA derivation.

Use `Reader::insert_metadata` with parent and relative to insert a known metadata
record explicitly. RPC nulls must be inserted explicitly with `insert_missing`;
an omitted account remains unknown. A missing metadata record gives empty labels;
metadata-dependent clients must include the metadata record to read its values.
