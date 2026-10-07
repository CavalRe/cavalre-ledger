# Transfer and recipient-creation costs — 2026-10-07

There is now **one Solana transfer handler**. The general Anchor `Transfer`
instruction is removed. Use `ledger_transfer::create_idempotent_instruction`
followed by `ledger_transfer::instruction` in the same transaction when a
recipient may be missing. Accounting remains one PDA per account.

## Recipient creation and transfer

Fresh sBPF measurements in the pinned LiteSVM runtime, ordered by decreasing
classic-token CU. Both token fixtures start with Alice holding 100 units, create
Bob's direct custodian leaf, and transfer 7. Compute includes authentication,
backing checks and the original posting events; setup and funding are excluded.

| Transaction | Classic token CU | Plain Token-2022 CU |
| --- | ---: | ---: |
| Create missing recipient + transfer | 32,031 | 32,046 |
| Create missing recipient only | 30,097 | 30,095 |
| Create existing recipient + transfer | 2,036 | 2,053 |
| Transfer only, recipient exists | 1,934 | 1,951 |
| Create existing recipient only | 102 | 102 |

The sample first receipt requires **1,851,360 lamports** of storage funding for
the leaf and parent-vector growth, apart from transaction fees. This is a local
runtime rent measurement, not a quoted cluster requirement. Obtain current rent
requirements from the target runtime.

First creation still uses the structural allocation host and canonical PDA
searches, so its cost depends on addresses, ancestry and supplied records. It is
not as cheap as moving balances between existing leaves. Repeated creation
checks only whether the recipient at fixed account position 4 has data. If so,
it returns immediately without hashes, decoding, writes or events. This no-op
does not certify account validity. Actual allocation and the following transfer
retain their full identity, authority and backing checks. A prefunded System-owned
PDA with empty data still takes the allocation path; a non-null RPC response alone
does not mean that a Ledger leaf has been initialized.

The previous no-op cost 1,015/1,032 CU because it hashed the same PDA twice and
repeated account, custody, authority and backing checks. Those checks now run
only where needed: during allocation or transfer. The always-include overhead
is 102 CU in both samples, without a preliminary RPC lookup.

The creation helper marks the parent writable so it can handle absence. Even
when the recipient exists, that transaction still acquires the declared parent
write lock. Omit creation for known existing recipients to avoid this contention.

A sponsor pays for storage but gains no authority over the recipient. A transfer
failure rolls back creation, the parent index and storage funding; the transaction
fee remains charged. Transfer alone rejects absent endpoints even for zero and
self-transfers. The removed Anchor transfer discriminator is rejected.

## Existing-account transfer shapes

These samples use existing leaves and the same optimized transfer byte format as
before this change. They include authorization, namespace checks, checked u128
arithmetic, backing and posting events, with no compute-budget instructions.
Numbers are local measurements, not throughput predictions or a balance-only
token-program comparison.

| Operation | CU |
| --- | ---: |
| Internal opposite-polarity issuance through the ledger | 8,183 |
| Internal debit transfer, unequal depths across branches | 6,286 |
| Internal credit transfer between application and Source | 4,235 |
| Native SOL, custodian siblings | 2,264 |
| Classic token, depth-4 siblings | 2,050 |
| Plain Token-2022, custodian siblings | 1,951 |
| Classic token, custodian siblings | 1,934 |
| Internal credit siblings at depth 4 | 1,823 |

The previous main revision measured 1,935/1,952 CU for the two token custodian
samples; transfer-only costs remain essentially unchanged. The under-1,000-CU
transfer target remains
unmet. The two 104-byte posting logs and two grouped SHA-256 calls alone cost
918 CU under the pinned runtime schedule.

Internal cases compare minimal and conservative account declarations from the
same pre-state, checking every supplied record and the full event sequence.
They exercise the same handler. There is no second general transfer fallback.
Equal-polarity siblings use the shared checked posting primitive at any depth;
other transfers use the shared ancestor walk, staging all changes before writes.

## Large parent vectors

The structural host reads fixed headers and the child slots it needs. Append
grows the inline vector by 32 bytes; removal swaps the last entry into the vacant
slot and shrinks the vector. Untouched child bytes are not decoded/serialized.

| Operation | 8 children | 256 children | 2,048 children | 4,096 children |
| --- | ---: | ---: | ---: | ---: |
| Remove with swap-and-pop | 33,585 | 33,585 | 33,585 | 33,585 |
| Create missing recipient + transfer | 29,223 | 29,223 | 29,223 | 29,223 |
| Add child | 22,382 | 22,414 | 22,643 | 22,906 |

The same first-receipt fixture previously cost **37,917 CU** through the removed
handler. Explicit creation plus optimized transfer is **22.9% lower** here.
This fixture uses different ancestry and addresses from the direct-custodian
creation table, so its creation cost differs.

The test seeds untouched sibling slots offchain, omits their individual records,
and verifies those bytes, reverse indexes, allocation, closure and double-entry
balances. Parent vectors remain unpaged: account loading, structural write
contention, account-size limits and runtime growth limits still apply.

## Reproduce and validation

Use pinned Rust 1.98.1, Agave 4.3.0 and platform-tools v1.57:

```sh
bash scripts/build-sbf.sh tests/consumer/Cargo.toml
bash scripts/build-sbf.sh crates/cavalre-ledger-solana/Cargo.toml
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked creation -- --nocapture
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked packed_transfers -- --nocapture
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked native_sol_lean_siblings -- --nocapture
cargo test -p cavalre-ledger-runtime-tests --test ledger --locked structural_writes_preserve_large_child_arrays -- --nocapture
```

Runtime cases cover sponsored creation, repeat no-ops, prefunded PDAs, rejected
identities/bumps/signers, internal authority, groups, protected Source, backing,
failed-transfer allocation rollback, readonly records, duplicate metas,
zero/self-transfers, malformed layouts and instruction lengths, metadata-PDA
substitution, Token-2022 options, native rent reserves, checked arithmetic,
ancestor overflow and application CPI. Existing-recipient creation has a
200-CU ceiling; the composed existing-recipient transaction has a 2,300-CU
ceiling. Custodian transfers retain a 2,150-CU ceiling and depth-4 debit siblings
retain a 2,250-CU ceiling.

The full gate is `bash scripts/check.sh`. The wider composed-workflow profile is
in [EXECUTION_LIMITS.md](EXECUTION_LIMITS.md); storage and account inputs are in
[READS.md](READS.md) and the [Solana README](../crates/cavalre-ledger-solana/README.md).
