# Solana acceptance coverage

The pinned `bash scripts/check.sh` gate builds the Ledger and test-consumer sBPF
artifacts before running tests. Runtime tests load those artifacts into LiteSVM;
they do not substitute native Rust handlers. This is local runtime validation,
not a deployed-cluster test or a security audit.

There are 38 test functions: 15 core tests (including eight independent host
tests), six Solana address/effective-flag tests, and 17 runtime tests.
One core test replays all 162 saved Solidity posting
cases, including expected rejections and every node's resulting gross balances.

## Exercised requirements

The added cases are in [acceptance.rs](../tests/runtime/acceptance.rs). The
original runtime scenarios are in [ledger.rs](../tests/runtime/ledger.rs).

| Area | Runtime evidence |
| --- | --- |
| Application authentication | An application PDA creates its branch through CPI; another program cannot sign for it; its administrator wallet cannot substitute for it when withdrawing. |
| Distinct token payer | The application PDA and a separate token owner authorize funding together. Missing payer signatures and a generic SPL delegation do not authorize the deposit. Withdrawal needs no recipient signature. |
| Tree authority | Another branch cannot be created, captured, mutated or removed by an unrelated authority. Applications cannot create external-token credits or mutate reserved Source metadata. |
| Transfer boundaries | Cross-custodian destinations, credit/group endpoints, wrong-ledger accounts, unauthorized callers and underfunded self-transfers reject. An authorized funded self-transfer preserves balances. |
| Account lifecycle | Registering a funded implicit debit leaf preserves its balance; repeated matching registration is idempotent; conflicting metadata, unauthorized repetition, funded group conversion and nonempty removal reject. Empty removal updates child counts. |
| Implicit leaves | Receipt allocates storage without registering the receiver or requiring its signature. Removing a funded implicit leaf is a no-op. Prefunding a PDA does not capture it or block allocation. |
| Parent admission | Registered-only parents reject implicit endpoints in deposits, withdrawals and both transfer directions, including zero amounts. A registered subgroup can allow its own implicit children. |
| Parent validation | No-op removal still requires a registered group parent in the selected ledger. A valid no-op removal remains allowed beneath a registered-only parent. |
| Account authentication | Wrong addresses, owners, headers, lengths, duplicate records and omitted parents reject without partial state changes. |
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
- Query/event compatibility remains implementation work. Shared account
  lifecycle, admission, authority and settlement policy now live in the core.
- Practical depth limits, compute usage, transaction size, rent costs and
  concurrency have not been characterized. Instructions still lock the root
  writable.
- Production identity, upgrade authority, deployment tooling and a deployed
  release rehearsal remain unfinished.
- Token-2022, direct native SOL, cross-custodian transfers, off-chain intents,
  authority recovery and EVM execution are outside the implemented scope.

The product specification still needs reconciliation with concrete Solana
interfaces, including accounting-only root ownership and distinct-payer signer
accounts. Passing this suite does not mark those open documentation and release
items complete.
