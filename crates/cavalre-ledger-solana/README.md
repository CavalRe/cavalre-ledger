# CavalRe Ledger on Solana

Ledger uses one accounting PDA per node and an independent optional metadata PDA.
The account's address is its storage address. Parent and custodian links can be
followed directly; there is no logical-address translation or container search.

The shared `no_std` core owns the debit/credit walk, checked arithmetic, custody
permissions, settlement rules and semantic events. The Solana adapter owns PDA
identity, runtime signers, packed storage, allocation and token/System CPI.
This is a development draft, with no deployed-layout migration provided.

## Storage

See [the exact layout](../../docs/READS.md). Ordinary records contain parent PDA,
custodian PDA, kind, depth, `u128` debit and credit, one-based `child_index`, and
the vector of relative child identities. A leaf takes 106 bytes. Ledger records
add asset kind, identifier, internal authority and vault before the vector.

Accounting seeds are `cavalre.ledger.account`, parent PDA and relative identity.
Metadata uses `cavalre.ledger.metadata` with the same parent and relative. Each
has its own canonical PDA bump. Metadata stores the accounting PDA's `bump`,
decimals, name and symbol; transfers do not require metadata.

There is no stored format, relative identity, ledger pointer, registration bit,
implicit-admission policy or accounting bump in ordinary accounting records.
The namespace selects the layout. A future incompatible layout needs a new
namespace. Ledger records are children of the distinguished global Root.
External mint addresses remain asset configuration, not ledger addresses.

Creating a funded leaf also appends its relative identity to the parent's vector.
Empty removal swaps the last child into the removed slot, updates that child's
index, and closes the removed accounting and optional metadata accounts. Supply
the metadata PDA even when it is absent, so labels cannot survive closure and
later attach to a recreated account. Rent from closed accounts returns to payer.

## Instructions and account inputs

Anchor structural and settlement contexts call the ledger account `ledger`.
`AddLedger`, `AddExternalToken` and `AddNativeSol` initialize a ledger and its
protected credit Source. Ledger creation also needs writable global Root.

`AddSubAccount`, `AddSubAccountGroup` and their named variants accept a parent
storage PDA and a relative identity (or derive the identity from the name).
`implicit_allowed` is no longer an instruction parameter. Metadata can be
omitted on creation. An authorized add can attach the first label later without
changing accounting or the child index; existing labels are immutable.

`Transfer` remains the structural service instruction: it may allocate an absent
endpoint and index it under its parent. Supply both endpoint PDAs, their parents,
custodians and needed ancestors. New records and changed parent vectors must be
writable. Existing common ancestors whose balances cancel remain read-only.
`Reader::transfer_writable_accounts` plans accounting write permissions from a
consistent supplied snapshot, including explicitly recorded absent endpoints.

For transfers between existing leaves, use `ledger_transfer::instruction`.
This client helper supplies canonical endpoint bumps in instruction data.
The `p-token-entrypoint` build uses Pinocchio parsing and direct fixed-offset
balance access. It authenticates endpoints with the accounting namespace and
supplied bumps, then follows stored custodian and parent PDAs. It never searches
for an ancestor address or an endpoint bump on-chain.

Its account order is authority signer, ledger, source, destination, then required
custodian/ancestor records and the external vault. Additional `AccountMeta`s must
mark changed ancestors writable; duplicate privileges are merged by the helper.
Sibling transfers need no parent for accounting; a custodian may still be needed
for authority verification. Equal-polarity siblings use the same core arithmetic
at every depth. Different-parent and opposite-polarity transfers use the original
shared walk. Only changed balances are written; zero transfers preserve events,
and self-transfers still check authorization and public debit funds.

Public external/native transfers authorize the source custodian. A recipient
custodian does not sign. Internal ledgers use their configured authority and
allow credit/debit postings. External callers cannot post to Source or create
credit claims. Relative identities may be on-curve or off-curve; an identity is
not a signature. Application authorities use `invoke_signed` through CPI.

## Native custody

`Wrap`/`Unwrap` settle classic SPL and compatible Token-2022 tokens;
`WrapSol`/`UnwrapSol` settle native SOL. The funding owner signs deposits in
addition to the accounting authority. Withdrawal recipients need not sign.
Amounts crossing a token/System boundary are `u64`; internal balances are `u128`.

Every external/native mutation checks live vault backing against outstanding
claims. Vault identity is pinned in the ledger configuration. Custody operations
validate actual before/after native balances and roll back accounting together
with token movement on failure. Vault donations do not create claims.

Supported Token-2022 extensions include metadata, display-unit extensions,
transfer hooks and permissioned burns. Fees, confidential transfers and other
unsupported balance behavior reject. Mint/account extension checks run during
settlement; ordinary transfers also validate the supplied vault. Hook account
resolution forwards only the hook's declared accounts. Application custody CPI
can use `ledger_cpi::set_custody_ledger_writable` to request ledger writes exactly
when the encoded amount is nonzero; outer privileges must permit them.

## Reads, events and verification

`default-features = false` excludes mutation handlers while retaining decoding
and read helpers. RPC can slice fixed fields. On-chain consumers can borrow the
bytes or use `Reader` without calling Ledger. Metadata must be supplied separately
when requested. Namespace identity must be checked separately from byte shape.

Events retain the original Credit/Debit meaning and ordering, with actual storage
PDAs in address fields. Both entrypoints emit the same event payloads. Index only
successful transactions; logs from a failed transaction are speculative.
See [events](../../docs/EVENTS.md).

Run `bash scripts/check.sh` with the pinned Agave toolchain on PATH. It builds
fresh Ledger and consumer sBPF and runs formatting, Clippy, read-only builds,
core fixture replays and runtime tests. [Transfer costs](../../docs/TRANSFER_COSTS.md)
and [execution limits](../../docs/EXECUTION_LIMITS.md) describe measured samples,
not universal compute or throughput guarantees. Parent vectors are unpaged, and
creation still needs parent write access and storage funding.
