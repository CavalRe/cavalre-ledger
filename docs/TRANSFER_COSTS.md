# Transfer cost profile — 2026-10-06

The optimized packed-PDA implementation uses **1,930 CU** for the sampled
classic-token custodian transfer (down from 2,740, **29.6% less**) and **1,947 CU**
for plain Token-2022 (down from 3,031, **35.8% less**). Equal-polarity siblings
one level deeper use **2,048 CU** (down from 2,788, **26.5% less**).
The earlier under-1,000-CU target is not met.

These are complete single-instruction transactions against fresh Ledger sBPF,
with authorization, namespace checks, checked u128 arithmetic, live backing
checks, and both posting events included. Setup and funding occur before the
measurement. There are no compute-budget instructions in these lean samples.
They are local measurements using the pinned LiteSVM environment, not throughput
predictions or a direct comparison with a token program's balance-only operation.

## Reproduce

Use Rust 1.98.1, Agave 4.3.0 and platform-tools v1.57:

```sh
bash scripts/build-sbf.sh tests/consumer/Cargo.toml
bash scripts/build-sbf.sh crates/cavalre-ledger-solana/Cargo.toml
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked packed_transfers -- --nocapture
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked native_sol_lean_siblings -- --nocapture
```

The fixtures use existing leaves with 100 units each and move 7 units. The
custodian fixture has two direct ledger children; the depth-4 fixture has two
children of one application custodian. Both leave the ledger and vault unchanged;
the depth-4 fixture also checks the complete custodian account is unchanged.
The only accounting write locks are the two endpoints. Metadata is omitted.

Rows are ordered by decreasing current lean-transfer CU. “Before” is the
previous packed-PDA implementation, with the same checks and events.

| Existing-account operation | Current lean CU | Before CU | Reduction | Structural service CU |
| --- | ---: | ---: | ---: | ---: |
| Internal opposite-polarity issuance through the ledger | 8,181 | 9,611 | 14.9% | 86,019 |
| Internal debit transfer, unequal depths across branches | 6,284 | 7,604 | 17.4% | 80,895 |
| Internal credit transfer between application and Source | 4,233 | 5,184 | 18.3% | 53,544 |
| Native SOL, custodian siblings | 2,260 | — | — | — |
| Classic token, depth-4 siblings | 2,048 | 2,788 | 26.5% | — |
| Plain Token-2022, custodian siblings | 1,947 | 3,031 | 35.8% | — |
| Classic token, custodian siblings | 1,930 | 2,740 | 29.6% | — |
| Internal credit siblings at depth 4 | 1,821 | — | — | 51,695 |

The four internal comparisons restore the exact same pre-state and run both
entrypoints. They assert equality of every supplied accounting record and the
full sequence of event bytes, not just endpoint balances. The structural service
still authenticates the supplied hierarchy, reads fixed headers and supports allocation;
the lean path accesses existing records through typed PDA links. These are
specific transaction shapes, not universal depth-dependent cost formulas.

## In-place child writes

The structural service now reads only fixed headers and the indexed child slots
it needs. Append grows the trailing array by 32 bytes; removal swaps the last
entry into the removed slot and shrinks the array. Untouched child bytes are
neither decoded into a heap vector nor serialized again. The layout and core
accounting rules are unchanged.

Fresh sBPF measurements, ordered by decreasing maximum CU:

| Operation | 8 children | 256 children | 2,048 children | 4,096 children |
| --- | ---: | ---: | ---: | ---: |
| First receipt | 37,917 | 37,917 | 37,917 | 37,917 |
| Remove with swap-and-pop | 33,579 | 33,579 | 33,579 | 33,579 |
| Add child | 22,377 | 22,409 | 22,638 | 22,901 |

The test seeds untouched sibling slots offchain, omits their individual accounts,
and verifies their bytes remain unchanged. It also verifies new and swapped
reverse indexes, allocation, closure, and double-entry balances. Runtime
loading/resizing and CPI costs can still depend on account size.

The previous binary failed with heap exhaustion when adding at 2,048 children.
At 8 children its first receipt/remove/add costs were 37,586/32,752/22,291 CU:
this change removes the heap/scaling failure, with a small constant overhead in
these small-parent fixtures. Existing-account lean transfers are unchanged.

Reproduce with `cargo test -p cavalre-ledger-runtime-tests --test ledger --locked
structural_writes_preserve_large_child_arrays -- --nocapture` after building
fresh sBPF. The full `scripts/check.sh` gate passes, including 80 runtime tests.

## Optimization changes

- Stack-based Pinocchio deserialization replaces heap allocation and push loops,
  retaining the runtime's duplicate-account handling and full account capacity.
- Admission keeps checked data borrows and reuses validated endpoint/custodian
  records. A shared custodian is checked once. Account keys use fixed-width word
  comparisons, including inside the generic ancestor walk.
- Existing leaves use exact-size structural checks. The adapter loads only the
  affected u128 column; checked arithmetic remains in the shared core. The debit
  subtraction performs the funds check instead of rereading the source first.
  Public self-transfers still check available funds.
- Each endpoint hashes the identical PDA preimage using three contiguous slices
  instead of six seed descriptors. Namespace, seeds, bump semantics and addresses
  are unchanged; both hashes remain mandatory.
- The sibling path reuses one event buffer for the two original log calls.
  Ancestor staging and extended vault parsing use separate call frames.
- A base-only Token-2022 vault validates the same initialized state, identities
  and COption tags without copying an unused token-account struct. Extended
  accounts retain the SPL extension parser and the existing allowlist.

No accounting layout, instruction ABI, authority rule, backing requirement or
posting event was removed or changed. Siblings at every depth use the same
accounting path; deeper samples still need their supplied custodian validated.

Under the pinned runtime's cost schedule, the two 104-byte posting logs cost
608 CU and the two grouped SHA-256 calls cost 310 CU. Together those syscalls
alone consume **918 CU**, before parsing, validation, arithmetic or event
preparation. This explains why these preserved semantics do not approach the
balance-only p-token benchmark.

## Storage design retained

Accounting identity is the actual storage PDA. Endpoint bumps come from the
client helper, permitting one namespace hash per endpoint without bump search.
Parents and custodians are stored PDAs and need no address derivation during the
walk. Authority and backing checks still require their corresponding accounts.
The fixed-offset layout keeps strings and metadata out of transfer parsing.

Equal-polarity siblings use the same checked core posting primitive regardless
of depth or custodian role. Other paths use the shared ancestor walk and stage
all balance changes before writing. Different-polarity siblings still propagate
both columns through their ancestors, as required by double-entry accounting.

The existing-account helper is `ledger_transfer::instruction`, enabled by the
program's `p-token-entrypoint` build feature. The regular Anchor `Transfer` remains
available for first receipt/allocation and structural service workflows. New
recipients require allocation, rent funding and a parent-vector write, so the
lean existing-recipient numbers do not apply to first receipt.

## Validation and remaining limits

Runtime cases cover bad relative identities/bumps, substitute authorities,
readonly endpoints, protected external Source, funded and underfunded
self-transfers, duplicate account metas, read-only zero transfers, malformed
instruction lengths and leaf layouts, same-owner metadata-PDA substitution,
underbacking, malformed Token-2022 option tags, native rent reserves, credit
sibling overdrafts, ancestor overflow rollback and application CPI. Custodian
samples enforce a 2,150-CU ceiling and depth-4 debit siblings a 2,250-CU ceiling
in the pinned runtime to catch regressions.
Both paths preserve Credit/Debit event encoding and order. The broad structural
resource profile is described in [EXECUTION_LIMITS.md](EXECUTION_LIMITS.md).

Ordinary leaves take 106 bytes; parent vectors contain 32 bytes per child and
metadata is separate. Transfers between independent existing siblings no longer
share a balance-container write lock. Creation still writes the parent vector.
Those vectors remain unpaged, so large-group loading, structural contention and
account-size/growth limits still matter. New namespaces and packed bytes replace
the undeployed draft format; there is no in-place migration provided.
