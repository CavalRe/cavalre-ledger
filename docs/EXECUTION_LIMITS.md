# Solana execution measurements — 2026-10-07

Ledger stores parent and custodian PDAs directly and uses the packed layout in
[READS.md](READS.md). There is no resource-based depth cap in the program. Depth
is a checked u8; applications must fit their complete transactions within compute,
heap, account and packet budgets. This sampled profile is not a universal limit.

## Reproduction and scope

Use the pinned Rust 1.98.1, Agave 4.3.0, platform-tools v1.57 and LiteSVM 0.16.0.
`bash scripts/check.sh` builds fresh Ledger and consumer sBPF and runs the profile.
For a focused rerun after building, run:

```sh
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked execution_profiles
```

`target/execution-profile.json` records 2,205 measured signed transactions across
leaf depths 4–13, three deterministic address sets and internal, external-direct
and application-CPI modes. All samples pass with the default 32 KiB heap. The
compute limit is 300,000 CU with a checked 10% margin; it is not a suggested
production budget. Legacy packets are used when they fit 1,232 bytes; otherwise
the test creates and warms an address lookup table and executes a v0 transaction.
ALT setup compute, fees and rent are recorded separately.

This is a **complete workflow** profile, including explicit `CreateIdempotent`
plus optimized transfer for missing endpoints, repeat transfers, names/metadata,
settlement and removal. Transfer-only samples and existing-recipient creation
are measured separately in [TRANSFER_COSTS.md](TRANSFER_COSTS.md).
The profile includes emitted events and backing checks. Internal ledgers use
64-byte names, external ledgers use 32-byte issuer names, and groups/explicit
leaves use 64-byte names. Initial ledger setup is outside the measured operations.

## Measured envelope

| Leaf depth | Maximum CU | Maximum packet bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 13 | 181,978 | 1,232 | 29 | 24 |
| 12 | 169,560 | 1,201 | 27 | 22 |
| 11 | 160,507 | 1,229 | 25 | 20 |
| 10 | 141,321 | 1,161 | 23 | 18 |
| 9 | 131,503 | 1,093 | 21 | 16 |
| 8 | 122,046 | 1,025 | 19 | 14 |
| 7 | 111,232 | 957 | 18 | 12 |
| 4 | 92,419 | 793 | 15 | 7 |
| 5 | 89,893 | 826 | 16 | 8 |
| 6 | 78,333 | 889 | 17 | 10 |

Rows are ordered by decreasing maximum CU. Each column is an independent maximum across that depth's operations. These are
local runtime measurements, not cluster throughput. PDA bump searches on the
structural path make costs address-dependent. A separate shorter workflow creates
and posts at depth 14, showing why depth alone is not an execution limit.

The structural host reserves its loaded-record buffer once and sorts references
to pending accounts, avoiding repeated full-record buffers under the bump allocator.
The optimized transfer borrows fixed fields and stages arithmetic before writing.
Transfer and settlement use the original shared ancestor walk; transfer derives
no ancestor PDAs. Creation still uses the structural host for missing records.
That host reuses canonical recipient derivations and skips unused metadata
address searches. The largest sampled workflow fell from 268,401 to 181,978 CU;
all 2,205 transactions retain their accounting, authority and event checks.

## Storage and write locks

Ordinary accounting records are 106 bytes plus 32 bytes per child. Ledgers are
203 bytes plus 32 bytes per child. Metadata is 10 bytes plus UTF-8 name and symbol.
Creation before first receipt creates a leaf and grows its parent's vector; later transfers
between existing siblings write only the two endpoints. There are no per-slot
accounts or shared containers for leaf balances.

Children vectors are unpaged. Large groups still incur account-loading costs,
parent write contention on structural operations, and limits on account growth.
Record sizes and metadata lengths determine rent funding; clients should obtain
current runtime rent requirements. Closing an empty child refunds its accounting
and metadata lamports to payer. Parent-vector shrinkage currently retains surplus
lamports in the parent account. Profile rent values are signed net changes and
include refunds. Fees and ALT setup costs are separate from storage funding.

Transfer itself never allocates, including zero and self-transfers, and requires
both endpoints to exist. Explicit recipient creation requires a writable parent
for indexing even when the following transfer amount is zero. Successful creation does not guarantee
that every later composed workflow will fit, so measure actual application
transactions including CPI, all additional instructions and address-table setup.
