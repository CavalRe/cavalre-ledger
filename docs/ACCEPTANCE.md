# Solana acceptance coverage

The pinned `bash scripts/check.sh` gate builds the Ledger and test-consumer sBPF
artifacts before running tests. Runtime tests load those artifacts into LiteSVM;
they do not substitute native Rust handlers. This is local runtime validation,
not a deployed-cluster test or a security audit.

There are 118 test functions: 35 core tests, 23 Solana library tests and
60 runtime tests. View suites also run with mutations excluded;
those repeat executions are not additional test functions.
One core test replays all 162 saved Solidity posting
cases, including expected rejections and every node's resulting gross balances.

## Exercised requirements

The added cases are in [acceptance.rs](../tests/runtime/acceptance.rs). The
original runtime scenarios are in [ledger.rs](../tests/runtime/ledger.rs).

| Area | Runtime evidence |
| --- | --- |
| Global Root | Internal, classic SPL, Token-2022 and native SOL ledgers are discovered through ordinary Root child queries. Failed first/later initialization, conflicting repeated creation, fake/readonly Root and child-count overflow reject atomically. Stale append-slot inputs reject atomically and succeed after refreshing the required slot. Posting leaves Root unchanged and does not need it as an input. |
| Application authentication | An application PDA creates its branch through CPI; another program cannot sign for it; its administrator wallet cannot substitute for it when withdrawing. |
| Distinct token payer | The application PDA and a separate token owner authorize funding together. Missing payer signatures and a generic SPL delegation do not authorize the deposit. Withdrawal needs no recipient signature. |
| Tree authority | Another branch cannot be created, captured, mutated or removed by an unrelated authority. Applications cannot create external-token credits or mutate reserved Source metadata. |
| Transfer boundaries | Cross-custodian destinations, credit/group endpoints, wrong-ledger accounts, unauthorized callers and underfunded self-transfers reject. An authorized funded self-transfer preserves balances. |
| Account lifecycle | Registering a funded implicit debit leaf preserves its balance; repeated matching registration is idempotent; conflicting metadata, unauthorized repetition, funded group conversion and nonempty removal reject. Empty removal updates child counts. |
| Named identities | Keccak golden vector and exact UTF-8 validation; named and explicit creation produce identical records, child slots and events for every account kind. Repeated creation uses readonly Ledger accounts and charges no rent. Funded implicit registration, parent scoping, duplicate display labels, invalid names, conflicts and unauthorized direct/CPI operations preserve existing rules. Named Source lookup resolves the initialized protected credit account and supports issuance/retirement; named creation cannot bypass Source protection. |
| Implicit leaves | Receipt allocates storage without registering the receiver or requiring its signature. Removing a funded implicit leaf is a no-op. Prefunding a PDA does not capture it or block allocation. |
| Child indexes | Maintained insertion order, indexed pages without sibling records, one-based reverse indexes, swap-and-pop removal, slot reuse and atomic rejection of missing/readonly/forged inputs. Root uses the same slots. |
| Parent admission | Registered-only parents reject implicit endpoints in deposits, withdrawals and both transfer directions, including zero amounts. A registered subgroup can allow its own implicit children. |
| Parent validation | No-op removal still requires a registered group parent in the selected ledger. A valid no-op removal remains allowed beneath a registered-only parent. |
| Account authentication | Wrong addresses, owners, headers, lengths, duplicate records and omitted parents reject without partial state changes. |
| Native SOL | Direct and application-CPI round trips enforce separate custodian/payer authority and allow unsigned recipients. Fee payer/wallet aliasing works. Prefunding and donations create no claims; rent is excluded from backing and retained after full withdrawal. Invalid authority, admission, vault identity/owner/data and insufficient total backing reject. Late initialization, deposit and withdrawal failures roll back lamports, records and rent allocation. |
| Token-2022 | Plain and metadata mints settle through direct calls and application CPI. Immutable-owner wallets work; UI-scaled tokens settle in raw units. Unsupported mint/account extensions reject, including on subsequent settlement. Mixed token programs and frozen accounts reject; late commit failure rolls back settlement and allocation. |
| Native token identity | Wrong mints, wallets, vault PDAs, vault authorities, substituted token programs and wallet/vault aliasing reject. |
| Settlement atomicity | Frozen token accounts reject. A late commit failure after token movement and new-leaf allocation rolls back token balances, ledger writes and rent allocation. |
| Custody root access | Zero-amount classic SPL, Token-2022 and native SOL wrap/unwrap run with all Ledger records read-only, directly and through raw CPI with and without in-place preparation, for absent and funded endpoints. Nonzero unprepared calls with a read-only root reject at commit and roll back settlement/allocation; prepared calls reject missing outer write privileges before entering Ledger. The same calls succeed when root writes are supplied. Application PDA signing and remaining records are preserved; a signed but incorrect token funder still rejects. Preparation tests check all four encoded layouts, high-bit amounts, idempotence, unchanged data/other metas and malformed-input rejection without modifications. |
| Shared custody | Two application branches share one Source and root totals. Direct donations increase backing without claims; an empty payer cannot use existing custody surplus to fund a deposit. |
| Withdrawal backing | A vault that covers an individual withdrawal but not all recorded claims rejects that withdrawal. |
| Internal accounting | Authorized issuance and retirement support the full `u128` range; overflow rejects atomically and internal issuance changes no external-token claims. |

The new rejection helper asserts the expected error and compares every supplied
account before and after failure. Only the actual transaction fee may be lost;
the fee payer's remaining lamports, data and ownership are checked as well.
Malformed-record fixtures deliberately inject invalid state to exercise decoder
rejection; they do not imply another program can write Ledger-owned records.

The [independent host tests](../crates/cavalre-ledger-core/tests/host.rs) exercise
the same service without Solana dependencies. They verify authentication for
every command, distinct actor/payer identities, operation-bound funding terms,
lifecycle/admission rules, internal issuance and external Source protection.
Injected short settlement and late commit failure test the host's rollback
contract across both token movement and ledger writes. The unchanged Solana
runtime acceptance suite also passes through this shared service.

The read suites compile independently of the mutating `ledger` modules. They
exercise registered and implicit properties, restricted-parent visibility,
gross/net/supply semantics, confirmed absence versus missing input, enumeration
completeness and decoder rejection. The Solana fixture supplies only readonly,
nonsigner accounts and verifies that neither bytes nor lamports change. A runtime
test also reads actual persisted records after a withdrawal is rejected for
insufficient total backing.

Root view tests use the shared record decoder and verify that discovery returns
exactly the same results as Root child queries. They reject forged Root state,
duplicate input, invalid children and incomplete snapshots, and preserve page
bounds. The independent host verifies that Root's child count and ledger/Source
creation roll back together.

Metadata tests cover stored external, native SOL and explicit accounting-only
metadata; zero decimals; authenticated Metaplex/Token-2022 sources; pointer
precedence; malformed/spoofed sources; and missing, empty and overlong fields.
Runtime tests verify atomic rejection, stored snapshots after issuer updates,
and matching repeated internal, classic SPL, Token-2022 and native SOL creation
without writes, rent allocation or events. Conflicting metadata rejects. The
runtime consumer reads stored metadata using only the ledger record, without
invoking Ledger. Inline issuer updates execute the actual Token-2022 program.

Event tests preserve original Source/ledger initialization order, structural
registration/removal and silent no-ops. Core tests verify posting order, gross
columns through opposite-polarity ancestors, cancellation, zero amounts, self
transfers and rollback. The runtime suite decodes actual logs through direct and
application CPI paths with both token programs, and checks native SOL creation.
Failed commit and later-instruction failures demonstrate why consumers must
discard the complete failed transaction's events. See [events](EVENTS.md) for
the explicit Solana payload adaptations and log-delivery limitations.

The execution suite measures 2205 transactions across sampled depths, direct
calls, application CPI and internal debit/credit postings. It checks signed
packet size, compute headroom, first-use rent and final accounting balances.
A separate regression creates groups, converts an unregistered leaf into a
group, and posts to implicit leaves beyond the former depth cap, through direct
and CPI calls. Checked depth overflow and `u128` balance overflow remain covered;
see [execution measurements](EXECUTION_LIMITS.md).

The allocation regression uses maximum-length names and counts allocations on
the calling thread: repeated flags/custody/balance reads allocate nothing, and
a posting allocates only its single bounded change buffer. Solana uses borrowed
record metadata, direct balance/child-count updates and one working record set.
The original posting fixtures and token/ledger rollback tests run unchanged.

At `187e252`, the named-creation regression comparison against `28b190f` found
all 2,205 existing
profiled transactions retain their status, packet size, account/write counts,
fees and rent. Explicit leaf registration costs 2 additional CUs; explicit group
registration costs 2 fewer. Measured posting, deposit, withdrawal and removal
compute costs are unchanged. The program artifact grows from 395,384 to 401,312
bytes (+5,928). In the direct internal-root, 64-byte-name creation cases, choosing
the named instruction costs 159–164 CUs more than its explicit equivalent.
Those new-path measurements are examples, not a bound for every transaction.

The subsequent compile-time `Source` identity change, compared with `187e252`,
preserves all 2,205 profiled transaction outcomes, packet sizes, account/write
counts, fees and rent. Ordinary operations cost 3 additional CUs. Source-related
issuance/retirement and settlement deltas range from -17,997 to +3 CUs across the
sampled roots; a different Source PDA can require fewer bump-search attempts.
These address-dependent savings are not a general performance guarantee. The
program artifact grows by 48 bytes to 401,360 bytes. The Source name hash itself
is evaluated at compile time, with no runtime hashing charge.

Removing unconditional custody-root write constraints, compared with `e13037b`,
preserves all 2,205 profiled transaction outcomes, packet sizes, account/write
counts, fees and rent with the existing conservative client permissions.
Measured classic SPL deposit/withdrawal calls cost 2 fewer CUs; other sampled
operations are unchanged. The program artifact shrinks by 200 bytes to 401,160.
There is a client compatibility change: generated account metas (including
Anchor CPI wrappers) now mark the root read-only. Nonzero custody instructions
must explicitly supply writable root metas. The `ledger_cpi` preparation helper
selects the inner root permission from the amount; applications must supply
the required privileges in the outer transaction. Zero-amount tests separately exercise the newly allowed
read-only Ledger records with native vault/wallet writes retained.

The typed custody wrappers added in `e01f007` have been replaced with
`ledger_cpi::set_custody_root_writable`, which adjusts an encoded instruction in
place without allocating or re-encoding. Compared with `e01f007`, all 2,205
profiled transaction outcomes, packet sizes, account/write counts, fees and rent
are unchanged. Direct and internal compute costs are unchanged. Raw CPI cases
cost 6 additional CUs from the revised test dispatch, or 12 CUs above the
pre-helper `0b8ec4e` consumer. This dispatch distinguishes the two test paths;
it is not required in an application that always prepares its instruction.
Ledger's 401,160-byte program is byte-for-byte unchanged from `e01f007`.

The test consumer shrinks from 158,432 to 130,184 bytes (-28,248). Its growth
above the original 129,608-byte raw-forwarding consumer is now 576 bytes (0.44%),
compared with 28,824 bytes for the typed version. Ledger account storage and
rent requirements are unchanged.

The custody root-access tests compare raw forwarding and in-place preparation
in the same consumer, using the same application PDA and nested leaf:

| Asset / operation | Raw forwarding CUs | Prepared CUs | Increase |
| --- | ---: | ---: | ---: |
| Classic SPL deposit | 74,367 | 74,404 | 37 (0.050%) |
| Classic SPL withdrawal | 68,225 | 68,262 | 37 (0.054%) |
| Token-2022 deposit | 76,634 | 76,671 | 37 (0.048%) |
| Token-2022 withdrawal | 70,492 | 70,529 | 37 (0.052%) |
| Native SOL deposit | 85,399 | 85,439 | 40 (0.047%) |
| Native SOL withdrawal | 79,267 | 79,307 | 40 (0.050%) |

The previous typed path added 2,869–3,186 CUs (3.4–4.7%) over its raw path.
The replacement forwards the same encoded payload after checking its kind and
layout and setting the root permission. These are complete transaction
measurements for these fixtures, not a bound for every application. Reproduce
*after* the full gate has finished to avoid overlapping Cargo builds with
different feature sets:
`cargo test -p cavalre-ledger-runtime-tests --test ledger custody_requires_root_writes_only --locked -- --nocapture`.

The [Token-2022 suite](../tests/runtime/token2022.rs) executes against LiteSVM's
bundled Token-2022 11.0.0 sBPF. Its plain/metadata mints, metadata initialization,
immutable-owner accounts, supply and freeze/thaw operations use actual token
instructions. Additional extension/substitution fixtures inject state to test
admission and validation; they do not imply applications can alter token-owned
accounts. The depth/compute profile above still measures classic SPL Token;
it is not a Token-2022 or native SOL resource guarantee. The
[native SOL suite](../tests/runtime/native_sol.rs) executes Ledger and the System
Program, including program-signed vault withdrawals through the test consumer.

## Regression fixed

Removal previously returned success for an absent target before validating its
parent. The new test reproduced that behavior. Removal now calls the shared
effective-flags resolver before its no-op return, restoring the original
LedgerLib parent-group and ledger-membership checks. It deliberately does not
apply monetary admission policy to removal.

## Still outside this evidence

- This does not port every original Solidity test or establish complete spec
  conformance. Randomized operation sequences and broader adversarial review
  remain useful additional coverage.
- Core balance/account query semantics and the independent read boundary are
  implemented, including native/token symbol and decimals queries. Custom metadata
  formats and a complete Solana root-discovery client remain; see READS.md. Event semantics and compatibility decisions are
  documented in EVENTS.md. Shared account lifecycle,
  admission, authority and settlement policy live in the core.
- Arbitrary deeper workflows, additional CPI layers, versioned/batched transactions
  and cluster throughput are not established by these measurements. Minimal-write
  runtime tests cover read-only common ancestors, equal/unequal-depth debit paths,
  credit paths, opposite-polarity mint/burn, zero/self transfers with existing or
  absent storage, CPI and tree mutations. No-op tests verify event contents,
  unchanged records, no endpoint allocation and fee-only payer debits. Zero
  custody operations cover classic SPL Token, Token-2022 and native SOL.
  Each planned record write is tested for atomic
  rejection when omitted. Shared non-Ledger writable accounts can still serialize
  transactions.
- Production identity, upgrade authority, deployment tooling and a deployed
  release rehearsal remain unfinished.
- Unsupported Token-2022 extensions, cross-custodian transfers, off-chain
  intents, authority recovery and EVM execution remain outside the implemented scope.

The product specification still needs reconciliation with concrete Solana
interfaces, including accounting-only root ownership and distinct-payer signer
accounts. Passing this suite does not mark those open documentation and release
items complete.
