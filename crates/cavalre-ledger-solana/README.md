# Solana Ledger draft

Fresh implementation based on the original `cavalre-contracts` LedgerLib at
`34d159ff4e88fdfdee16738d9a1228f0bf407212`. This crate does not use the previous
controller service or accounting kernel. This is the only active implementation.
The earlier prototype is available in Git history; see
[the porting notes](../../docs/PORTING.md) for reusable Solana pieces.

This first slice implements account identity, effective flags, ledger lookup
and custody resolution. It is a library, not yet an executable Solana program.
No storage layout, deployment identity or migration is introduced.

| Original Solidity | This draft |
| --- | --- |
| `toAddress(parent, relative)` | `to_address`: PDA derived from parent and relative public keys |
| Packed flags | Typed `Flags` with the same parent, kind, token kind and depth |
| Zero registration flags | `None`; independent of whether balance storage exists |
| `effectiveFlags` | `effective_flags`: registered metadata wins; implicit leaves inherit polarity and depth |
| `ledger` | Root identity or lookup through the registered custodian |
| `custody` | Direct relative identity or the parent's existing custodian |

Names and responsibilities follow Solidity so the implementations can be
reviewed side by side. `DebitLedger` and `CreditLedger` retain the original
names for leaves. Depth retains the original convention: token root at 2,
direct children at 3. There is no per-leaf controller or separate app token tree.

`Store` is a read interface for authenticated account data, not a second balance
store. Its eventual Solana implementation must verify ownership, PDA identity
and metadata before calling these helpers. Invalid data must not become an
implicit leaf. Callers must resolve effective flags before custody lookup;
these helpers do not authenticate signers or enforce spending permissions.

Solana must allocate storage when a balance first needs persistence. That does
not require registering the logical ledger leaf or obtaining its owner's
signature. Allocation and its rent payer will be handled by the instruction
layer; neither is implemented in this slice. Parent admission restrictions are
also a separate, explicit addition to the original behavior.

Next: bring over the original `transfer` ancestor walk and its debit/credit
behavior, then implement authenticated Solana storage and instructions.
Source remains an explicit credit account in the intended tree. Token custody,
app authorization and token compatibility checks are not implemented yet.

Run `cargo test -p cavalre-ledger-solana --locked` from the repository root.
The tests exercise inherited debit and credit leaves, registered overrides,
custody ancestry, root mismatch, invalid parents and depth overflow.
