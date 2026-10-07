# Solana acceptance coverage

`bash scripts/check.sh` checks formatting and Clippy, runs the mutation-disabled
library suites, builds fresh Ledger and consumer sBPF, and tests the workspace.
The latest gate passes 82 runtime tests, including the 2,205-transaction resource
profile. Runtime tests use those binaries in the pinned LiteSVM runtime. This is local
runtime validation, not a deployed-cluster test or security audit.

The unchanged saved Solidity fixtures cover 162 posting steps and 138 custody
steps. Posting replay checks every gross balance and rejection. Custody replay
runs through both the independent core host and actual sBPF using classic SPL
and Token-2022. The 13 expected custody failures, donations, native balances,
claims, Source and backing remain checked. Fixtures and reference sources are
comparison material, not implementation dependencies.

## Current layout and behavior

| Area | Evidence |
| --- | --- |
| Packed storage | Exact offsets, little-endian u128 limits, truncation, malformed vectors, owner checks, ledger identity and independent metadata namespace tests. Ordinary leaves are 106 bytes. |
| Namespace authentication | Invalid endpoint bump/relative/PDA, malformed packed leaves and same-owner accounting-shaped data at a metadata PDA reject. A valid metadata string that resembles an accounting header is still classified by its namespace. |
| Global Root | Internal, classic SPL, Token-2022 and native ledgers appear in the inline child vector. Initialization, repetition, failed append and malformed Root cases are atomic. |
| Lifecycle | `CreateIdempotent` creates/indexes default leaves when recipient data is empty; otherwise it immediately skips allocation. The no-op does not certify validity. Following transfers reject bad identities/bumps, malformed records, groups, protected external Source, unauthorized spending and underbacking. Actual creation still validates identity, authority and backing. Sponsored external recipients retain custody; prefunded empty PDAs work. Matching adds are idempotent; first labels may attach later. Empty removal performs swap-pop and closes accounting/metadata storage. |
| Authority | Application CPI signers, distinct funding owners, branch isolation, external Source protection, rejected substitute authorities, and unsigned recipients. Public transfers authorize only the source custodian. |
| Sibling transfers | Direct custodian-to-custodian and deeper siblings use only endpoint balance writes. Ledger/common-parent/vault snapshots remain unchanged. No custodian-specific accounting branch. Debit and credit sibling paths, duplicate metas, funded read-only self-transfers and read-only zero transfers are covered; sampled CU ceilings catch regressions. |
| Ancestor walk | Minimal and conservative account declarations produce identical full account states and exact event payloads through the sole transfer handler for unequal depths, credit paths and opposite-polarity postings. Stored parent PDAs are followed directly. |
| Atomic failure | A failing transfer rolls back preceding recipient creation, index writes and storage funding. Bad identity, readonly writes, arithmetic overflow, failed settlement and failed late commit preserve all supplied account states apart from transaction fees. The removed Anchor transfer discriminator rejects; transfer cannot create missing endpoints, including zero/self cases. |
| Native custody | Classic SPL, compatible Token-2022 and native SOL round trips, actual settlement checks, insufficient backing freezes and recovery by donations without minting claims. |
| Token-2022 | Metadata pointers and inline metadata, hooks, permissioned burns, supported display extensions, rejected incompatible extensions, malformed option tags in base-only vaults, and changed mint/vault configuration. |
| Metadata and reads | Optional separate labels, mint-authenticated snapshots, inherited unit metadata, missing versus omitted records, read-only consumers, owner/namespace checks and unchanged input bytes. |
| Events | Source/ledger initialization order, lifecycle no-ops, Credit/Debit direction and gross-column meaning, zero/self behavior, direct/CPI equivalence and speculative failed-transaction logs. |
| Resource profile | 2,205 signed transactions at sampled leaf depths 4–13, with direct/internal/CPI paths, default heap and v0/ALT packets where needed; a shorter depth-14 workflow also succeeds. |

See [READS.md](READS.md) for storage, [PORTING.md](PORTING.md) for intentional
Solana adaptations, [TRANSFER_COSTS.md](TRANSFER_COSTS.md) for the lean transfer
measurements, and [EXECUTION_LIMITS.md](EXECUTION_LIMITS.md) for the wider profile.

The core retains a host-independent logical lifecycle; Solana allocation and
indexing are adapted at commit. There is no Solana persisted registration policy,
separate child-slot account, stored relative/ledger field or leaf container map.
Passing saved fixtures does not establish full original-suite or audited parity.

The separate `bash scripts/reference.sh` regeneration check passes with pinned
Foundry 1.8.3: all 162 hierarchy cases and 138 custody steps match the saved
fixtures. Reference sources and fixtures were not modified.
