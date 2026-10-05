# Solana execution limits

The current Solana adapter permits **leaf depth 13 and group depth 12**. The root
has depth 2: a deepest leaf is eleven parent-child edges below it. A group must
leave one level for usable children, whether those children are registered or
implicit. These limits apply to both external-token and accounting-only trees.

The adapter exports `MAX_ACCOUNT_DEPTH` and `MAX_GROUP_DEPTH` in `ledger_lib`.
Its host rejects out-of-range creation and metadata writes with `DepthLimit`, including conversion of
an empty unregistered leaf into a group. Rejection rolls back the entire
transaction apart from its fee. Account encoding and instruction arguments are
unchanged. The reusable core has no new platform-specific depth cap, and read
decoding does not apply this mutation limit.

## Why this limit exists

With this implementation, all measured operations succeed through depth 13.
An exploratory build admitting depth 14 reached the legacy packet-size boundary:
the first distant-branch transfer required 1234 bytes for internal accounting,
1297 bytes for direct external-token calls, and 1267 bytes through the consumer.
Those transactions were rejected by the profiler's 1232-byte wire-size check
before submission. The sweep found no heap failures before that boundary.

The earlier depth-7 cap in `014919b` addressed avoidable heap consumption. The
adapter cloned its entire loaded state, and even simple flags/balance queries
cloned account names. The correction keeps the original posting walk and
permissions while changing storage access:

- Reads borrow names from validated records, without allocating metadata.
- Balance and child-count writes update only those fields.
- The Solana adapter keeps one working record set and marks changed entries.
- Record and posting buffers reserve their bounded capacity once.

The same 32 KiB allocator and runtime rollback are used. At depth 7, peak measured
compute fell from 181059 to 139490 CU, approximately 23%. The owned snapshots are
gone; no custom allocator, larger heap request or account-layout change was needed.
The allocation regression checks zero allocations for repeated field and numeric
queries, and exactly one change-buffer allocation for a posting.

Depth 13 is the tested envelope of the current legacy transaction construction,
not a Solana protocol depth restriction. At its largest, a measured transaction
uses 1231 of 1232 bytes. Additional instructions or accounts may require shallower
paths or a separately tested transaction format. Increasing the cap requires
demonstrating usable complete operations, not just successful group creation.

## Measured envelope

Run on 2026-10-04 using Rust 1.98.1, Agave 4.3.0, platform-tools v1.57 and
LiteSVM 0.16.0, with freshly built Ledger and test-consumer sBPF. The runtime
dependencies and feature configuration are those pinned in Cargo.lock and
LiteSVM's default setup. This is local execution evidence, not cluster throughput.

The regression performs **2205 measured transactions** across 90 scenarios:
leaf depths 4 through 13, three deterministic address sets, and three invocation
modes: internal accounting, direct external-token calls, and an application PDA
calling through the test consumer. Roots, groups and registered endpoints use
the maximum 64-byte names. The external direct deposit has separate fee payer,
application authority and token payer; the CPI path uses its own program-signed
authority and a separate token payer. Internal paths diverge at the root;
external paths diverge immediately below the shared application group.

| Leaf depth | Maximum compute units | Maximum transaction bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 4 | 109731 | 793 | 15 | 7 |
| 5 | 92408 | 826 | 16 | 8 |
| 6 | 109227 | 859 | 17 | 10 |
| 7 | 139490 | 892 | 18 | 12 |
| 8 | 151913 | 925 | 19 | 14 |
| 9 | 159970 | 967 | 20 | 16 |
| 10 | 186992 | 1033 | 22 | 18 |
| 11 | 191701 | 1099 | 24 | 20 |
| 12 | 201100 | 1165 | 26 | 22 |
| 13 | 214153 | 1231 | 28 | 24 |

Each column is its own maximum across that depth's measured operations. Compute
need not increase monotonically because PDA bump searches vary with addresses.
These are sampled measurements, not universal worst-case CU promises.

The tests submit signed **legacy transactions**, including both compute-limit
and compute-price instructions. They enforce the 1232-byte packet limit before
calling LiteSVM; a successful simulator execution alone would not establish wire
admissibility. They request 300000 CUs and require at least 10% headroom in every
sample. No lookup tables, custom heap, disabled signature checks, or native
substitute for the Ledger program are used. Mint/wallet fixtures are installed
as test setup; all Ledger tree state is created through actual instructions.

| Operation at depth 13 | Maximum compute units | Maximum bytes | New storage funding (SOL) |
| --- | ---: | ---: | ---: |
| Create group | 94872 | 858 | 0.0044544 |
| Create registered leaf | 90122 | 890 | 0.0044544 |
| First deposit / Source issuance | 154114 | 1090 | 0.0044544 |
| Repeated deposit / Source issuance | 147668 | 1090 | 0 |
| First transfer to implicit leaf | 205998 | 1231 | 0.0044544 |
| Repeated transfer | 199455 | 1231 | 0 |
| Transfer between registered leaves | 199549 | 1231 | 0 |
| Withdraw / retire to Source | 146098 | 994 | 0 |
| First issuance from deep credit leaf | 214153 | 1168 | 0.0044544 |
| Issuance from registered deep credit leaf | 207741 | 1168 | 0 |
| Retirement to deep credit leaf | 207838 | 1168 | 0 |

The complete generated report includes registration and removal measurements.
The suite verifies final leaf, Source, root and custody balances. A separate
boundary test rejects excessive group depth for all three invocation modes,
including an attempt to reuse an allocated, unregistered leaf. It checks the
specific error and rollback of every supplied account, including payer funds
after the transaction fee.

## Funding and client construction

Each new 512-byte Ledger record requires 0.0044544 SOL in the pinned simulator's
rent configuration. First receipt pays this once without registering the leaf;
later movements and registration of that existing record require no new rent.
Unregistration retains the record and does not refund rent. The test compares
the actual payer debit, minus transaction fees, with the runtime's minimum
balance for every new record.

At the same rent setting, root plus Source funding is 0.0089088 SOL; adding a
165-byte classic SPL vault gives 0.01094808 SOL for external-ledger storage.
Those initialization totals are calculated from account sizes, not timed samples
in the table. Mint creation, wallet creation, application state and transaction
fees are separate. Clients must query the target cluster's
`getMinimumBalanceForRentExemption` rather than hard-code these amounts.

Clients should simulate the complete transaction, add compute headroom, verify
its serialized size including signatures, and supply the measured account paths.
An application can add enough instructions, accounts or CPI overhead to exceed
limits even when its Ledger tree fits. The sample 300000-CU budget is a test
setting, not a fixed fee recommendation. Versioned transactions, additional CPI
layers and batched operations need their own measurements.

## Concurrency

All current mutation contexts require the root writable. Consequently, mutations
to different application branches of the same token still share a write lock.
Deposits and withdrawals also write the shared Source and vault. Independent
roots can avoid those shared locks, but a shared writable payer or token wallet
can still serialize transactions. The account counts above are measured; no
TPS, scheduling latency or cluster contention claim is made. Removing unnecessary
root locks is separate optimization work.

## Reproduce

With the pinned tools on PATH:

```bash
bash scripts/profile-solana.sh
```

This rebuilds both programs, runs the profile and boundary tests, prints binary
hashes and summary tables, and writes individual samples to
`target/execution-profile.json`. The same tests run under `bash scripts/check.sh`.
Increasing the constants requires rerunning the complete envelope, including
opposite-polarity internal postings and first-use leaves, without weakening the
resource or accounting assertions.

Protocol references: [legacy transaction structure](https://solana.com/docs/core/transactions/transaction-structure),
[compute and heap budgets](https://solana.com/docs/core/fees/compute-budget), and
[rent minimum RPC](https://solana.com/docs/rpc/http/getminimumbalanceforrentexemption).
The measured format is legacy; this report does not rely on activation of newer
transaction formats or cluster feature gates.
