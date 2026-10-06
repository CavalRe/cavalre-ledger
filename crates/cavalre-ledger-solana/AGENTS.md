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

## Agreed storage update (2026-10-06)

The user's agreed Solana layout supersedes earlier registration/storage guidance
above: all identities and links are accounting storage PDAs, children are an
inline relative-address vector, and allocation atomically creates/indexes an
account. No persisted registered/implicit_allowed state, stored relative or
ledger pointer, account bump, or per-child index PDA. Metadata is separate and
optional. Debit and credit balances remain u128. See docs/READS.md and
PORTING.md at the repository root for details. Do not resume Float work while
finishing Ledger.
