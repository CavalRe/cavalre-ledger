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
coverage is separate from this resource profile. Accounting roots, groups and registered endpoints use 64-byte names; classic
SPL roots use Metaplex's maximum 32-byte names. Accounting symbols use four bytes
and classic SPL symbols use three bytes. The external direct deposit has separate fee payer,
application authority and token payer; the CPI path uses its own program-signed
authority and a separate token payer. Internal paths diverge at the root;
external paths diverge immediately below the shared application group.

| Leaf depth | Maximum compute units | Maximum transaction bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 4 | 84681 | 793 | 15 | 7 |
| 5 | 91058 | 826 | 16 | 8 |
| 6 | 116997 | 859 | 17 | 10 |
| 7 | 129838 | 892 | 18 | 12 |
| 8 | 139169 | 925 | 19 | 14 |
| 9 | 133413 | 967 | 20 | 16 |
| 10 | 145577 | 1033 | 22 | 18 |
| 11 | 183518 | 1099 | 24 | 20 |
| 12 | 182745 | 1165 | 26 | 22 |
| 13 | 208438 | 1231 | 28 | 24 |

Each column is its own maximum across that depth's measured operations. Compute
need not increase monotonically because PDA bump searches vary with addresses.
These are sampled measurements, not universal worst-case CU promises. The
measurements include the current structural and debit/credit events emitted
through Anchor logs and validation of the global Root parent; see [event delivery](EVENTS.md).
Ledger creation is setup outside this profile. Only creation writes the global
Root child index and count; measured postings do not need Root as an account input.

The adapter avoids three sources of unnecessary work:

- Successful account lookups construct no Anchor error objects; errors are
  allocated only when a lookup fails.
- Endpoint derivation reuses a loaded record's authenticated parent, relative
  identity and address. Loading still validates each stored PDA, and new records
  still use canonical derivation. Unknown children require a fresh derivation.
- External-token registration calls the shared mint and metadata decoders
  directly, without constructing the general-purpose Reader's four map indexes.
  Metadata ownership, canonical source and issuer-pointer selection still apply.

Against the saved `f6cb304` profile, all 2205 transactions use fewer CUs, saving
1822–69275 CU per transaction. The largest sampled transaction falls from 241286
to 208438 CU (13.6%). The direct depth-7 repeat transfer that previously used
155647 CU now uses 86372 CU. Packet lengths, account/write counts, fees, rent,
operation results and final accounting checks are unchanged. Initial PDA bump
searches remain address-dependent, so these samples are not universal bounds.

The Ledger executable falls from 404048 to 364952 bytes, saving 39096 bytes
(9.7%). Isolating the metadata change removes 37440 bytes. The freshly rebuilt
test consumer grows from 132528 to 133648 bytes (+1120, 0.85%); it is a test
fixture, not the Ledger deployment. Record sizes and rent are unchanged.

Isolated experiments distinguish the effects: lazy error construction saves
1236–15141 CU per sample; reusing authenticated addresses then saves another
8755–64331 CU on repeated transfers. The address lookup alone adds up to 112 CU
on some creation paths because it scans the supplied working records before
falling back to derivation. The combined changes more than offset that cost in
every sampled transaction. These measurements exclude the experimental
all-mutation backing check.

The tests submit signed **legacy transactions**, including both compute-limit
and compute-price instructions. They enforce the 1232-byte packet limit before
calling LiteSVM; a successful simulator execution alone would not establish wire
admissibility. They request 300000 CUs and require at least 10% headroom in every
sample. No lookup tables, custom heap, disabled signature checks, or native
substitute for the Ledger program are used. Mint/wallet fixtures are installed
as test setup; all Ledger tree state is created through actual instructions.

| Operation at depth 13 | Maximum compute units | Maximum bytes | New storage funding (SOL) |
| --- | ---: | ---: | ---: |
| Create group | 131732 | 891 | 0.00590208 |
| Create registered leaf | 110299 | 923 | 0.00590208 |
| Register funded leaf | 103658 | 923 | 0.00144768 |
| Remove registered leaf | 94519 | 853 | 0 |
| First deposit / Source issuance | 161325 | 1090 | 0.0044544 |
| Repeated deposit / Source issuance | 133556 | 1090 | 0 |
| First transfer to implicit leaf | 204404 | 1231 | 0.0044544 |
| Repeated transfer | 187820 | 1231 | 0 |
| Transfer between registered leaves | 187894 | 1231 | 0 |
| Withdraw / retire to Source | 136575 | 994 | 0 |
| First issuance from deep credit leaf | 208438 | 1168 | 0.0044544 |
| Issuance from registered deep credit leaf | 166938 | 1168 | 0 |
| Retirement to deep credit leaf | 167023 | 1168 | 0 |

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
setting, not a fixed fee recommendation. Versioned transactions, additional CPI
layers and batched operations need their own measurements.

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
The measured format is legacy; this report does not rely on activation of newer
transaction formats or cluster feature gates.
