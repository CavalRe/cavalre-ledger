# Solana execution measurements — 2026-10-06

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

This is the **structural Anchor service** profile, including initial allocation,
repeat transfers, names/metadata, settlement and removal. The lean existing-account
entrypoint is measured separately in [TRANSFER_COSTS.md](TRANSFER_COSTS.md).
The profile includes emitted events and backing checks. Internal ledgers use
64-byte names, external ledgers use 32-byte issuer names, and groups/explicit
leaves use 64-byte names. Initial ledger setup is outside the measured operations.

## Measured envelope

| Leaf depth | Maximum CU | Maximum packet bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 13 | 265,587 | 1,168 | 29 | 24 |
| 12 | 250,225 | 1,198 | 27 | 22 |
| 11 | 239,702 | 1,132 | 25 | 20 |
| 10 | 202,516 | 1,066 | 23 | 18 |
| 9 | 182,169 | 1,000 | 21 | 16 |
| 8 | 165,162 | 934 | 19 | 14 |
| 7 | 131,313 | 892 | 18 | 12 |
| 6 | 103,656 | 859 | 17 | 10 |
| 5 | 96,490 | 826 | 16 | 8 |
| 4 | 91,739 | 793 | 15 | 7 |

Rows are ordered by decreasing maximum CU. Each column is an independent maximum across that depth's operations. These are
local runtime measurements, not cluster throughput. PDA bump searches on the
structural path make costs address-dependent. A separate shorter workflow creates
and posts at depth 14, showing why depth alone is not an execution limit.

The structural host reserves its loaded-record buffer once and sorts references
to pending accounts, avoiding repeated full-record buffers under the bump allocator.
The lean transfer borrows fixed fields and stages arithmetic before writing. Both
use the original shared ancestor walk; the lean path derives no ancestor PDAs.

## Storage and write locks

Ordinary accounting records are 106 bytes plus 32 bytes per child. Ledgers are
203 bytes plus 32 bytes per child. Metadata is 10 bytes plus UTF-8 name and symbol.
A first receipt creates a leaf and grows its parent's vector; later transfers
between existing siblings write only the two endpoints. There are no per-slot
accounts or shared containers for leaf balances.

Children vectors are unpaged. Large groups still incur account-loading costs,
parent write contention on structural operations, and limits on account growth.
Record sizes and metadata lengths determine rent funding; clients should obtain
current runtime rent requirements. Closing an empty child refunds its accounting
and metadata lamports to payer. Parent-vector shrinkage currently retains surplus
lamports in the parent account. Profile rent values are signed net changes and
include refunds. Fees and ALT setup costs are separate from storage funding.

Zero movements and funded self-transfers allocate nothing; first nonzero receipt
requires a writable parent for indexing. Successful creation does not guarantee
that every later composed workflow will fit, so measure actual application
transactions including CPI, all additional instructions and address-table setup.
