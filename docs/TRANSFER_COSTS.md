# Transfer cost profile — 2026-10-06

The logical address correction and mapping layout are implemented. The
**under-1,000-CU Alice-to-Bob target is not met**. Existing recipients cost
16,345 instruction CU in the direct classic-token sample below, versus 30,536
before this work and 105 for the runtime's external checked token transfer.
This is 46.5% below the original Ledger path but still about 156 times the token
instruction's CU. It is not evidence of readiness to deploy.

## Reproduce

Use Rust 1.98.1, Agave 4.3.0, platform-tools v1.57 and the pinned LiteSVM 0.16.0:

```sh
bash scripts/build-sbf.sh crates/cavalre-ledger-solana/Cargo.toml
cargo test -p cavalre-ledger-runtime-tests --test ledger profile_mapped_transfer --locked -- --nocapture
```

`target/transfer-costs.json` contains CU, actual simulator fees, rent deltas,
serialized transaction bytes, writable keys and runtime logs. The program is
fresh sBPF, not a native substitute. Setup/registration/deposits are outside the
measured transfer. The mint is deterministic `[110; 32]`, decimals 6, metadata
`Reward`/`RWD`; the app group is `App` under Alice's authority. Alice starts with
100,000 internal raw units and Bob with 1. Both are implicit leaves. The measured
transfer moves 10. First receipt then sends 10 to a third, absent leaf. Self and
zero cases run after those transfers. The external recipient's token record
already exists. The exact token wallet address does not match the older SR
harness, but authority, mint, metadata, app and internal economic state do.

## Actual uninstrumented costs

All rows use one signature, a 200,000 CU limit and no priority fee. Transaction
CU includes 150 CU for setting the compute limit; instruction CU excludes it.
A limit is not a measured cost or a recommendation for production settings.

| Operation | Instruction CU | Transaction CU | Packet bytes | New rent (SOL) |
| --- | ---: | ---: | ---: | ---: |
| External checked token transfer | 105 | 255 | 319 | 0.00000000 |
| Ledger, existing recipient | 16,345 | 16,495 | 496 | 0.00000000 |
| Ledger, first receipt | 19,501 | 19,651 | 496 | 0.00200448 |
| Ledger, self transfer | 11,557 | 11,707 | 496 | 0.00000000 |
| Ledger, zero transfer | 15,209 | 15,359 | 496 | 0.00000000 |

Each row pays **5,000 lamports = 0.000005 SOL** in transaction fees in this
runtime. CU is not charged per consumed unit in these no-priority-fee samples.
If a client sets a compute-unit price, priority fees depend on its requested
limit and price. New storage rent is a separate funded balance, not a CU fee;
Ledger currently has no rent-reclamation operation.

The existing-recipient packet has seven total keys and two writable keys: payer
and app container. The token root and custody are readonly. The external packet
has six keys and three writable keys. Self/zero Ledger calls have only the payer
writable and allocate nothing. They retain authorization/backing checks; zero
posting retains its original events.

| Existing-recipient implementation | Instruction CU | Packet bytes |
| --- | ---: | ---: |
| Before work (`9d74900`), leaf PDA identities/storage | 30,536 | 562 |
| Interim hash-only validation and fixed-root cleanup, still leaf PDAs | 16,171 | 562 |
| Plain logical child hash and mapped leaves | 16,345 | 496 |

**Regression against the interim version:** +174 CU (1.1%). Mapping substantially
reduces first-leaf rent and account metas, but does not itself make the whole
transfer cheap. The interim measurement was a temporary stage, not a deployed
version. The initial compact mapping prototype measured 17,722 CU in a nearby
fixture; removing redundant physical lookups and growth checks reduced it.

## Where the existing transfer spends CU

A separate instrumented build measured stage boundaries with runtime CU logs.
Each marker adds two logging syscalls. The approximate figures below subtract
200 CU per marker interval; logging and compiler changes prevent these estimates
from summing exactly to the uninstrumented total. **16,345 CU above is the actual
production-build measurement; this table is attribution, not another benchmark.**

| Work | Approximate CU |
| --- | ---: |
| Anchor dispatch, fixed account handling, return | 1,950 |
| Decode/authenticate token root and app group | 2,940 |
| Host setup, live custody observation, shared entry checks | 1,270 |
| Derive/select/decode two mapped leaves | 2,580 |
| Resolve effective endpoints, custody authority and available funds | 3,460 |
| Posting plan, ancestor cancellation and implicit-leaf checks | 2,070 |
| Stage the two changed balances | 610 |
| Emit Credit and Debit events | 840 |
| Commit mapped fields | 760 |

This still contains avoidable adapter and repeated-resolution overhead. The
plain packed 64-byte Keccak has a 117-CU syscall charge in the pinned runtime,
plus caller/setup instructions. That is not the 2,580-CU leaf-loading row, which
also covers mapping search, decoding, state lookup and allocation of working
records. There is no off-curve validation for logical leaf addresses. The fixed
global Root is a constant. Runtime-owned group containers validate their stored
physical bump with a hash, not repeated curve search. New physical group creation
still follows Solana PDA allocation rules.

No authorization, effective flags, backing check, event or checked posting was
removed to obtain these numbers. The core accounting source is unchanged.
The next performance work is targeted reduction of repeated decoding/resolution
through the existing core, measured against this benchmark. Replacing the
accounting model to hide this overhead would invalidate the comparison.

## Storage and other regressions

- **Shared write locks:** leaves under one parent occupy one physical container.
  Sibling transfers contend even when the parent's own balance cancels unchanged.
  Separate subgroup containers can still be independent. Direct SR holders share
  the SR root container, which must now be writable for their transfers.
- **Finite, unpaged containers:** a container is 528 base bytes plus 288 bytes per
  capacity entry. Solana's 10 MiB account limit permits at most 36,407 entries in
  this layout, including group references and retained unregistered entries.
  Global Root has the same limit for ledger references. Other execution limits
  may bind earlier. No paging or unlimited-growth claim is made.
- **Growth cost:** binary lookup is logarithmic; insertion can move existing
  bytes and reallocation moves the child vector. Insertion is not constant-cost
  at large cardinalities. The mutation path decodes only requested leaves, but
  the convenience Reader loads supplied containers into its indexes. RPC clients
  currently fetch whole containers instead of individual leaf or index records.
- **Group rent increases:** an empty group container plus a new parent reference
  costs 0.00657024 SOL in the pinned runtime, versus 0.00590208 SOL for the old
  group plus child slot (+0.00066816 SOL, 11.3%). A new mapped leaf costs
  0.00200448 SOL, versus 0.00445440 SOL for an implicit standalone leaf (55% less),
  or 0.00590208 SOL for a registered leaf with its old child slot. Retained capacity
  can be reused without growth rent; removal does not refund it.
- **Binary size:** 436,368 bytes versus 414,776 before work, +21,592 bytes (5.2%).
  The mapping encoder/decoder and container lifecycle add code despite removing
  leaf/index allocation. This is a deployment-storage increase.
- **Draft format/API change:** child identities and storage layout changed.
  `to_address` now takes only parent/relative and returns one logical address.
  Clients supply group containers, not leaf/index PDAs. Group storage helpers
  remain separate. There is no existing deployment to migrate.

## Verification

`bash scripts/check.sh` passes the 141-test workspace suite, the separate
mutation-disabled suites, Clippy, formatting and fresh Ledger/consumer sBPF
builds. It replays the unchanged 162 hierarchy steps and 138 custody steps;
the custody fixture also executes against classic SPL and Token-2022. Targeted
coverage includes removed-group/implicit-leaf/re-registration transitions,
no-op allocation, relative/PDA signer authority, backing freezes, rollback,
child insertion/swap-pop and readonly ancestors where physical storage permits.

The standalone SR consumer's 10 tests pass against this layout, including its
unchanged 139 staking and 40 self/cross-SR fixture steps. Its full workspace gate
is checked separately. Solidity fixture regeneration could not run here because
Foundry 1.8.3 is unavailable; fixture files and reference sources were not edited.
Passing the Rust replay does not substitute for regenerated Solidity execution.

The broader profile executes 2,205 signed transactions at leaf depths 4–13,
including internal, external and application-CPI paths. Every sample succeeds
with the default heap and fits legacy packets; these are samples, not universal
worst-case bounds. The previous maximum was 137,185 CU; it is now 105,078 CU.

| Leaf depth | Maximum CU | Maximum packet bytes | Maximum account keys | Maximum writable keys |
| --- | ---: | ---: | ---: | ---: |
| 4 | 42350 | 727 | 13 | 5 |
| 5 | 46059 | 760 | 14 | 6 |
| 6 | 49802 | 793 | 15 | 8 |
| 7 | 53599 | 826 | 16 | 10 |
| 8 | 60643 | 868 | 17 | 12 |
| 9 | 69035 | 934 | 19 | 14 |
| 10 | 77611 | 1000 | 21 | 16 |
| 11 | 86498 | 1066 | 23 | 18 |
| 12 | 95695 | 1132 | 25 | 20 |
| 13 | 105078 | 1198 | 27 | 22 |
