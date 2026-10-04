# Ledger core

Platform-independent rules based on the original Solidity `LedgerLib.sol` at
`34d159ff4e88fdfdee16738d9a1228f0bf407212`. This crate is `no_std`, has no runtime dependencies. Posting changes use `alloc::Vec` so arithmetic
failures do not partially mutate host storage. It does not use the removed controller
implementation.

`src/ledger_lib.rs` retains the original names and responsibilities in Rust
style: account kinds, flags, `effective_flags`, `ledger` and `custody`.
Registered metadata takes precedence; implicit leaves inherit their parent's
polarity and depth without registration. Custody resolution identifies an
account's custodian; it does not authorize spending.

Hosts provide:

- An address type supporting copy and equality.
- `AddressDerivation`: deterministic, domain-separated parent/relative identity.
- `Store`: authenticated flags, custody and relative-address reads.

The host must verify its storage and identities before exposing them through
`Store`; corrupt data must return an error rather than appear unregistered.
Signer verification, storage allocation, serialization and token transfers
remain outside this crate. Future mutations must commit atomically with host
state and external settlement.

The Solana adapter supplies PDA derivation and public keys. The tests here use
an independent in-memory host and test-only integer identities, without a
Solana SDK. Those test identities are not a production address scheme.

The original posting arithmetic and ancestor walk live in `transfer`.
`transfer_debits` enforces same-custodian debit transfers and checks funds before
self-transfer no-ops. Hosts must commit returned balance changes atomically
against the same state. The Solana program supplies account lifecycle, native
signer validation and token settlement. There is no separate kernel crate.

Balances and posting amounts use checked `u128`. Native SPL movement uses `u64`;
wide internal values never truncate into native token transfers.

```bash
cargo test -p cavalre-ledger-core --locked
cargo tree -p cavalre-ledger-core
```
