# Solana execution measurements

Ledger imposes no resource-based depth cap. Applications are responsible for
ensuring their complete transactions fit the runtime's compute, heap, account
and transaction-size limits, including all instructions and CPI calls. A depth
cap cannot guarantee that a composed transaction will fit its shared budget.

Depth remains a checked `u8`, with root depth 2. Overflow, malformed trees,
unauthorized operations and invalid accounting still reject. Ledger records remain 512 bytes; child indexes use separate 80-byte slots. Posting instruction arguments are unchanged. Resource exhaustion fails execution;
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
The backing guard adds a read-only custody account to ordinary external-ledger
mutations. Eighteen depth-13 distant-branch samples now exceed the legacy packet
limit and use tested v0 transactions with address lookup tables. Additional
instructions or accounts still require measuring the complete transaction.
A separate regression succeeds at depth 14 with a shorter
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
coverage is separate from this resource profile. Accounting roots, groups and registered endpoints use 64-byte names; classic
SPL roots use Metaplex's maximum 32-byte names. Accounting symbols use four bytes
and classic SPL symbols use three bytes. The external direct deposit has separate fee payer,
application authority and token payer; the CPI path uses its own program-signed
authority and a separate token payer. Internal paths diverge at the root;
external paths diverge immediately below the shared application group.

| Leaf depth | Maximum compute units | Maximum transaction bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 4 | 57191 | 793 | 15 | 7 |
| 5 | 62182 | 826 | 16 | 8 |
| 6 | 71080 | 859 | 17 | 10 |
| 7 | 81268 | 892 | 18 | 12 |
| 8 | 85171 | 934 | 19 | 14 |
| 9 | 93997 | 1000 | 21 | 16 |
| 10 | 104512 | 1066 | 23 | 18 |
| 11 | 118215 | 1132 | 25 | 20 |
| 12 | 126106 | 1198 | 27 | 22 |
| 13 | 137185 | 1168 | 29 | 24 |

Each column is its own maximum across that depth's measured operations. Compute
need not increase monotonically because PDA bump searches vary with addresses.
These are sampled measurements, not universal worst-case CU promises. The
measurements include the current structural and debit/credit events emitted
through Anchor logs and validation of the global Root parent; see [event delivery](EVENTS.md).
Ledger creation is setup outside this profile. Only creation writes the global
Root child index and count; measured postings do not need Root as an account input.

The adapter reduces work without changing the core accounting walk:

- Runtime records authenticate their stored canonical bump with one PDA
  derivation. Initialization still searches for the canonical bump, and the raw
  byte decoder used by Reader still checks canonical derivation. Root and child
  records with corrupted bumps reject before settlement.
- Loaded child identities, newly derived endpoints and new child-slot bumps are
  reused within the instruction. Only results of authenticated loading or
  canonical derivation enter the temporary cache; caller-supplied keys do not.
- New accounts with zero lamports use one System Program create-account CPI.
  Prefunded PDAs retain the top-up/allocate/assign path, so prefunding cannot
  prevent legitimate allocation.
- Balance, child-count and reverse-index updates write their encoded fields
  directly. Metadata changes and new records use full serialization. Encoding
  tests compare the field writes against complete Borsh records byte for byte,
  including variable-length names, symbols and maximum integer values.
- Token balances use Anchor's initial decode and reload after the transfer CPI.
  The core still verifies actual before/after wallet and vault settlement.
- Successful lookups construct no errors, and token registration uses shared
  metadata decoders without constructing the full Reader's map indexes.

Before the backing guard, the optimization in `b146860` reduced the largest
sample from 208438 to 136116 CU. The current profile includes the backing guard
on every external/native mutation and reaches 137185 CU. The complete profile
and depth-14 workflow still use the default 32 KiB heap. The Borsh record payload,
instruction arguments, account sizes and Ledger allocation rent are unchanged.
The authenticated custody address occupies 32 previously unused bytes in each
external/native root; ordinary calls no longer search for its PDA.

All samples include compute-limit and compute-price instructions. The profiler
enforces the 1232-byte packet limit before calling LiteSVM; successful simulator
execution alone would not establish wire admissibility. It requests 300000 CUs
and requires at least 10% headroom in every sample. Of 2205 signed transactions,
2187 use legacy encoding and 18 use v0 with a lookup table. Signature checks
remain enabled. Lookup tables are created and extended through the actual
lookup-table program, then warmed by advancing the simulator clock; their setup
fees, compute and storage funding are recorded separately. Mint/wallet fixtures
are installed as test setup; Ledger state is created through actual instructions.

### Backing check cost and client impact

Comparison with the saved `b146860` profile, matching all 2205 samples in order:

Pinning the authenticated vault address at creation removes the repeated PDA
search from ordinary backing checks. Against `a7f4106`, these calls save
1479–3364 CU. The short direct transfer drops from 37026 to 33662 CU; its backing
check overhead falls from 3953 CU (12.0%) to 589 CU (1.8%). The largest CPI transfer
drops from 140531 to 137185 CU; its overhead falls from 4415 to 1069 CU.
Across sampled ordinary SPL transfers/tree changes, the remaining increase is
0.49–3.73%, with all vault owner, mint, authority, layout and extension checks
retained. The observed balance is always fresh.

This change adds 1568 executable bytes over `a7f4106` (0.42%); the consumer is
unchanged. The new immutable binding is written during initialization, whose
compute is outside this profile. It uses existing storage, with no added rent.
All sampled packet sizes, account/write counts, fees and allocation rent match
`a7f4106`, including its 18 v0/lookup-table cases. The one read-only vault account
is still required. Current total overhead relative to the pre-freeze build:

| Sampled operation | Added CU |
| --- | ---: |
| Direct classic SPL transfer | 589–959 |
| Application-CPI classic SPL transfer | 962–1439 |
| Direct classic SPL tree mutation | 633–709 |
| Application-CPI classic SPL tree mutation | 1006–1135 |
| Classic SPL deposit | -267 to +155 |
| Classic SPL withdrawal | -108 to -54 |

Deposits and withdrawals reuse the already-validated custody observation;
withdrawals no longer run a separate withdrawal-only backing check. Accounting-only
samples change by -456 to +38 CU and require no custody account. These are sampled
costs for the stated paths, not bounds for Token-2022, native SOL or arbitrary trees.

Ordinary external-ledger mutations add one read-only vault account: **33 legacy
packet bytes**, no new writable account and no Ledger storage allocation. Existing
clients must include it, including CPI callers and zero/self/repeated calls.
The profile's depth-13 external transfers grow from 1231 to 1264 legacy bytes
directly, and from 1201 to 1234 through CPI. Their v0 encodings are 556 and 464
bytes respectively. Each tested lookup table costs 15000 lamports in setup fees
and 0.00707136 SOL (direct) or 0.0075168 SOL (CPI) in storage funding, then is reused
for three transfers without further setup. Clients may provision reusable tables;
these particular costs describe the test setup, not a per-transfer charge.

The largest sampled transaction rises by 1069 CU (0.79%). Ledger grows from
369232 to 378784 executable bytes (+9552, 2.59%); the test consumer grows from
135384 to 135392 bytes (+8). Ledger transaction fees, allocation rent and writable
key counts match the baseline in every sample; lookup-table setup is additional.
Compute charges with a nonzero priority price depend on the requested budget.

| Operation at depth 13 | Maximum compute units | Maximum bytes | New storage funding (SOL) |
| --- | ---: | ---: | ---: |
| Create group | 82801 | 924 | 0.00590208 |
| Create registered leaf | 81170 | 956 | 0.00590208 |
| Register funded leaf | 71547 | 956 | 0 or 0.00144768 |
| Remove registered leaf | 72316 | 886 | 0 |
| First deposit / Source issuance | 109653 | 1090 | 0.0044544 |
| Repeated deposit / Source issuance | 100657 | 1090 | 0 |
| First transfer to implicit leaf | 137185 | 1168 | 0.0044544 |
| Repeated transfer | 133845 | 1168 | 0 |
| Transfer between registered leaves | 133923 | 1168 | 0 |
| Withdraw / retire to Source | 100479 | 994 | 0 |
| First issuance from deep credit leaf | 135461 | 1168 | 0.0044544 |
| Issuance from registered deep credit leaf | 121657 | 1168 | 0 |
| Retirement to deep credit leaf | 121742 | 1168 | 0 |

The complete generated report includes registration and removal measurements.
The suite verifies final leaf, Source, root and custody balances. A separate
regression exercises all three invocation modes beyond the former cap: it
creates groups through depth 13, registers and converts an empty leaf into a
group at depth 14, funds an implicit leaf at depth 14, and transfers between
sibling implicit leaves. It verifies their balances and the Source/root totals.

## Funding and client construction

Each new 512-byte Ledger record requires 0.0044544 SOL in the pinned simulator's
rent configuration. Each new 80-byte child slot requires 0.00144768 SOL. A new
registered leaf or group therefore requires 0.00590208 SOL when its slot is also
new. First receipt allocates only the record and does not register the leaf;
registering it later may allocate a child slot. Repeated movements need no new
storage. Unregistration retains records and cleared slots; later appends reuse
those slots. The test compares the payer debit, minus fees, with the runtime's
minimum balance for each allocation's actual size.

At the same rent setting, a new ledger and Source plus their two child slots
require 0.01180416 SOL. A 165-byte classic SPL vault adds 0.00203928 SOL, for
0.01384344 SOL in external-ledger storage. The first ledger also allocates global
Root for 0.0044544 SOL. These initialization totals are calculated from account
sizes, not timed samples in the table. Mint creation, wallet creation, application state and transaction
fees are separate. Clients must query the target cluster's
`getMinimumBalanceForRentExemption` rather than hard-code these amounts.

Clients should simulate the complete transaction, add compute headroom, verify
its serialized size including signatures, and supply the measured account paths.
An application can add enough instructions, accounts or CPI overhead to exceed
limits even when its Ledger tree fits. The sample 300000-CU budget is a test
setting, not a fixed fee recommendation. Additional versioned transaction shapes,
CPI layers and batched operations need their own measurements.

## Concurrency

Transfer clients can supply unchanged ancestors, including the app group and
token ledger root, read-only. `Reader::transfer_writable_accounts` uses the core
posting walk to select changed records and absent endpoints requiring allocation.
Same-polarity paths stop below their lowest common ancestor. Opposite-polarity
postings change balances through the token root; nonzero deposits and withdrawals
also write the shared Source and vault. Zero-amount custody calls accept all
Ledger records read-only, but still require writable native vault/wallet accounts
and the fee payer for their unchanged transfer path. Tree mutations write parents whose child
counts change. Permissions must be selected before signing the transaction.

Independent branches can avoid a shared root write lock, but shared writable
payers, token wallets or other application state can still serialize transactions.
The backing guard reads the shared vault. Read-only custody observations coexist
across transfers and tree changes, but conflict with deposits, withdrawals and
direct top-ups that write that vault. A freeze check therefore adds a custody
read dependency even when no ancestor balance changes.
Existing profiling fixtures retain conservative writable declarations; the account
counts above are measured, but no TPS, scheduling latency or cluster contention
claim is made. Minimal write declarations and atomic rejection are exercised
separately in `tests/runtime/writable_accounts.rs`.

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
The measured formats are legacy and v0 with address lookup tables. The profile
does not establish compatibility with other transaction formats or feature gates.
