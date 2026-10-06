# Porting Ledger to Solana

The accounting baseline remains `cavalre-contracts/modules/ledger/LedgerLib.sol`
at commit `34d159ff4e88fdfdee16738d9a1228f0bf407212`. The immutable hierarchy and
custody fixtures remain comparison material. The shared `no_std` core owns
posting, checked arithmetic, permissions, exact settlement and semantic events.
The Solana adapter owns storage, PDA identity, signer verification, allocation,
token CPI and transaction rollback.

## Agreed Solana storage

See [READS.md](READS.md) for the exact packed layout. The accounting namespace is
`cavalre.ledger.account`; metadata uses `cavalre.ledger.metadata`. All tree links
are storage PDAs. Ordinary accounts store parent, custodian, kind, depth, two
u128 balances, child_index and the inline vector of relative child identities.
Only ledger records additionally store token configuration. Metadata, including
decimals and the accounting bump, is independent and optional.

These choices supersede the prior draft's logical-ID/storage-ID translation,
512-byte records, stored relative/ledger/bump/registration fields, and per-slot
child accounts. This is an undeployed draft with new PDA namespaces, not an
in-place migration of deployed accounts.

Solana allocation establishes the account and its index. The core's original
logical lifecycle remains available to other hosts; the Solana adapter indexes
newly materialized leaves at commit and closes empty removed accounts. There is
no Solana `implicit_allowed` instruction parameter. Metadata attachment is an
optional host capability; the default core host retains its previous behavior.

Public debit transfers authorize the source custodian. A recipient custodian
does not need to sign. Destination leaf kind, ledger membership, funds, backing,
and atomicity checks still apply. The internal-ledger authority controls its
accounting operations. Equal-polarity siblings update only their endpoints at
any depth. Opposite-polarity or different-parent transfers use the shared
ancestor walk and direct parent PDA links.

## Entry points

Structural operations and custody settlement use the existing Anchor service.
It reads only fixed accounting headers, may create missing leaves, and stages
indexed child changes. Commit resizes and writes those slots directly, without
decoding or serializing the parent's existing children. The program also
exports the lean existing-account transfer instruction through
`ledger_transfer::instruction`. It uses Pinocchio account parsing, fixed-offset
reads/writes, two namespace hashes with client-supplied bumps, and the same core
posting arithmetic. It never searches for bumps or derives ancestor addresses.

The helper's account order is signer, ledger, source, destination, followed by
required custodian/ancestor records and the external vault. Pass writable metas
for ancestors that will change; the helper merges duplicate privileges. Only
changed balances need write permission. Metadata is omitted. A self transfer
still authenticates and checks available public debit funds. Zero transfers
emit the original posting events without writing balances.

Ownership and namespace checks are mandatory: a supplied relative identity is
not authority. The no-curve-check hash is used only against records owned by this
program, which only creates canonical accounting PDAs. Initial creation still
uses canonical PDA derivation and allocation signer seeds.

## Verification

Run `bash scripts/check.sh` with the pinned Agave toolchain on PATH. It checks
formatting, Clippy, read-only builds, fresh sBPF builds of Ledger and the consumer,
and the full workspace tests. No deployment or crate publication is part of
this work. Float representation changes remain deferred in work/notes.
