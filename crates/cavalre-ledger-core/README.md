# Ledger core

Platform-independent rules based on the original Solidity `LedgerLib.sol` at
`34d159ff4e88fdfdee16738d9a1228f0bf407212`. This crate is `no_std`, uses no heap
allocation, and has no dependencies. It does not use the removed controller
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

The original posting arithmetic and ancestor walk will be added here next.
There is no separate kernel crate for now. Account mutations, permissions and
token settlement are not implemented in this slice.

```bash
cargo test -p cavalre-ledger-core --locked
cargo tree -p cavalre-ledger-core
```
