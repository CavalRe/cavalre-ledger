# Fresh Solana Ledger

- Use the original `cavalre-contracts/modules/ledger/LedgerLib.sol` at
  `34d159ff4e88fdfdee16738d9a1228f0bf407212` as the behavioral baseline.
- Preserve recognizable function names and responsibilities in Rust style.
- The previous controller service and separate namespace token trees are not
  requirements for this crate. Do not import their policy or assume parity.
- Preserve implicit leaves, effective flags, relative identities, custody
  ancestry, explicit Source and the original double-entry ancestor walk.
- Distinguish Solana storage allocation from logical account registration.
- This is a fresh draft, not an upgrade to the earlier program. Do not change
  existing program layouts or deploy anything as part of work on this crate.
- Document material behavioral differences and keep platform adaptations narrow.
