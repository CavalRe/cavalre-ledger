# Solana execution measurements

Ledger imposes no resource-based depth cap. Applications are responsible for
ensuring their complete transactions fit the runtime's compute, heap, account
and transaction-size limits, including all instructions and CPI calls. A depth
cap cannot guarantee that a composed transaction will fit its shared budget.

Depth remains a checked `u8`, with root depth 2. Overflow, malformed trees,
unauthorized operations and invalid accounting still reject. The stored format
and instruction arguments are unchanged. Resource exhaustion fails execution;
Solana rolls back the failed transaction's state changes apart from fees.
Successful account creation does not guarantee that every later operation on
that account will fit: applications must validate their intended workflows.

## Profiling scope

The full profile samples leaf depths 4 through 13. Earlier exploration at depth
14 reached the legacy packet-size boundary for distant-branch transfers:
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

The sampling range belongs only to the profiler; it is not enforced by Ledger.
At its largest, a measured transaction uses 1231 of 1232 bytes. Additional
instructions or accounts may require shallower paths or a separately tested
transaction format. A separate regression succeeds at depth 14 with a shorter
posting path, demonstrating why depth alone does not determine whether a
transaction fits.

## Measured envelope

Run on 2026-10-05 UTC using Rust 1.98.1, Agave 4.3.0, platform-tools v1.57 and
LiteSVM 0.16.0, with freshly built Ledger and test-consumer sBPF. The runtime
dependencies and feature configuration are those pinned in Cargo.lock and
LiteSVM's default setup. This is local execution evidence, not cluster throughput.

The regression performs **2205 measured transactions** across 90 scenarios:
leaf depths 4 through 13, three deterministic address sets, and three invocation
modes: internal accounting, direct classic SPL Token calls, and an application
PDA calling through the test consumer. Token-2022 and native SOL functional
coverage is separate from this resource profile. Roots, groups and registered endpoints use
the maximum 64-byte names. The external direct deposit has separate fee payer,
application authority and token payer; the CPI path uses its own program-signed
authority and a separate token payer. Internal paths diverge at the root;
external paths diverge immediately below the shared application group.

| Leaf depth | Maximum compute units | Maximum transaction bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 4 | 110406 | 793 | 15 | 7 |
| 5 | 92427 | 826 | 16 | 8 |
| 6 | 109514 | 859 | 17 | 10 |
| 7 | 139525 | 892 | 18 | 12 |
| 8 | 151965 | 925 | 19 | 14 |
| 9 | 160021 | 967 | 20 | 16 |
| 10 | 187051 | 1033 | 22 | 18 |
| 11 | 191768 | 1099 | 24 | 20 |
| 12 | 201175 | 1165 | 26 | 22 |
| 13 | 214247 | 1231 | 28 | 24 |

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
| Create group | 94868 | 858 | 0.0044544 |
| Create registered leaf | 90117 | 890 | 0.0044544 |
| First deposit / Source issuance | 154825 | 1090 | 0.0044544 |
| Repeated deposit / Source issuance | 148389 | 1090 | 0 |
| First transfer to implicit leaf | 206090 | 1231 | 0.0044544 |
| Repeated transfer | 199557 | 1231 | 0 |
| Transfer between registered leaves | 199651 | 1231 | 0 |
| Withdraw / retire to Source | 146201 | 994 | 0 |
| First issuance from deep credit leaf | 214247 | 1168 | 0.0044544 |
| Issuance from registered deep credit leaf | 207845 | 1168 | 0 |
| Retirement to deep credit leaf | 207942 | 1168 | 0 |

The complete generated report includes registration and removal measurements.
The suite verifies final leaf, Source, root and custody balances. A separate
regression exercises all three invocation modes beyond the former cap: it
creates groups through depth 13, registers and converts an empty leaf into a
group at depth 14, funds an implicit leaf at depth 14, and transfers between
sibling implicit leaves. It verifies their balances and the Source/root totals.

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

This rebuilds both programs, runs the profile and deeper-tree tests, prints binary
hashes and summary tables, and writes individual samples to
`target/execution-profile.json`. The same tests run under `bash scripts/check.sh`.
The sampled depths and budgets are test settings, not exported program limits.
Consumers should profile their own complete workflows, including first-use
allocation and opposite-polarity postings where applicable.

Protocol references: [legacy transaction structure](https://solana.com/docs/core/transactions/transaction-structure),
[compute and heap budgets](https://solana.com/docs/core/fees/compute-budget), and
[rent minimum RPC](https://solana.com/docs/rpc/http/getminimumbalanceforrentexemption).
The measured format is legacy; this report does not rely on activation of newer
transaction formats or cluster feature gates.
