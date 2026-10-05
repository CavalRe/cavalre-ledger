# Solana acceptance coverage

The pinned `bash scripts/check.sh` gate builds the Ledger and test-consumer sBPF
artifacts before running tests. Runtime tests load those artifacts into LiteSVM;
they do not substitute native Rust handlers. This is local runtime validation,
not a deployed-cluster test or a security audit.

There are 90 test functions: 31 core tests (including 14 independent host
tests and ten view tests), six Solana address/effective-flag tests, thirteen Solana
view/metadata/Root tests and 40 runtime tests. View suites also run with mutations excluded;
those repeat executions are not additional test functions.
One core test replays all 162 saved Solidity posting
cases, including expected rejections and every node's resulting gross balances.

## Exercised requirements

The added cases are in [acceptance.rs](../tests/runtime/acceptance.rs). The
original runtime scenarios are in [ledger.rs](../tests/runtime/ledger.rs).

| Area | Runtime evidence |
| --- | --- |
| Global Root | Internal, classic SPL, Token-2022 and native SOL ledgers are discovered through ordinary Root child queries. Failed first/later initialization, duplicate creation, fake/readonly Root and child-count overflow reject atomically. Stale append-slot inputs reject atomically and succeed after refreshing the required slot. Posting leaves Root unchanged and does not need it as an input. |
| Application authentication | An application PDA creates its branch through CPI; another program cannot sign for it; its administrator wallet cannot substitute for it when withdrawing. |
| Distinct token payer | The application PDA and a separate token owner authorize funding together. Missing payer signatures and a generic SPL delegation do not authorize the deposit. Withdrawal needs no recipient signature. |
| Tree authority | Another branch cannot be created, captured, mutated or removed by an unrelated authority. Applications cannot create external-token credits or mutate reserved Source metadata. |
| Transfer boundaries | Cross-custodian destinations, credit/group endpoints, wrong-ledger accounts, unauthorized callers and underfunded self-transfers reject. An authorized funded self-transfer preserves balances. |
| Account lifecycle | Registering a funded implicit debit leaf preserves its balance; repeated matching registration is idempotent; conflicting metadata, unauthorized repetition, funded group conversion and nonempty removal reject. Empty removal updates child counts. |
| Implicit leaves | Receipt allocates storage without registering the receiver or requiring its signature. Removing a funded implicit leaf is a no-op. Prefunding a PDA does not capture it or block allocation. |
| Child indexes | Maintained insertion order, indexed pages without sibling records, one-based reverse indexes, swap-and-pop removal, slot reuse and atomic rejection of missing/readonly/forged inputs. Root uses the same slots. |
| Parent admission | Registered-only parents reject implicit endpoints in deposits, withdrawals and both transfer directions, including zero amounts. A registered subgroup can allow its own implicit children. |
| Parent validation | No-op removal still requires a registered group parent in the selected ledger. A valid no-op removal remains allowed beneath a registered-only parent. |
| Account authentication | Wrong addresses, owners, headers, lengths, duplicate records and omitted parents reject without partial state changes. |
| Native SOL | Direct and application-CPI round trips enforce separate custodian/payer authority and allow unsigned recipients. Fee payer/wallet aliasing works. Prefunding and donations create no claims; rent is excluded from backing and retained after full withdrawal. Invalid authority, admission, vault identity/owner/data and insufficient total backing reject. Late initialization, deposit and withdrawal failures roll back lamports, records and rent allocation. |
| Token-2022 | Plain and metadata mints settle through direct calls and application CPI. Immutable-owner wallets work; UI-scaled tokens settle in raw units. Unsupported mint/account extensions reject, including on subsequent settlement. Mixed token programs and frozen accounts reject; late commit failure rolls back settlement and allocation. |
| Native token identity | Wrong mints, wallets, vault PDAs, vault authorities, substituted token programs and wallet/vault aliasing reject. |
| Settlement atomicity | Frozen token accounts reject. A late commit failure after token movement and new-leaf allocation rolls back token balances, ledger writes and rent allocation. |
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

Metadata tests cover native SOL, undefined internal metadata, zero decimals,
canonical Metaplex sources, Token-2022 pointer precedence, malformed/spoofed
sources and missing versus confirmed absent metadata. The runtime consumer reads
a mint initialized by the actual Token-2022 program using only readonly,
nonsigner query accounts, without invoking Ledger. After a token metadata update,
a fresh read returns the updated symbol while preserving mint and Ledger state.

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
  formats, accounting-only metadata configuration, a complete Solana root-discovery
  client remain; see READS.md. Event semantics and compatibility decisions are
  documented in EVENTS.md. Shared account lifecycle,
  admission, authority and settlement policy live in the core.
- Arbitrary deeper workflows, additional CPI layers, versioned/batched transactions
  and cluster throughput are not established by these measurements. Instructions still
  lock the root writable, serializing mutations that share a root.
- Production identity, upgrade authority, deployment tooling and a deployed
  release rehearsal remain unfinished.
- Unsupported Token-2022 extensions, cross-custodian transfers, off-chain
  intents, authority recovery and EVM execution remain outside the implemented scope.

The product specification still needs reconciliation with concrete Solana
interfaces, including accounting-only root ownership and distinct-payer signer
accounts. Passing this suite does not mark those open documentation and release
items complete.
