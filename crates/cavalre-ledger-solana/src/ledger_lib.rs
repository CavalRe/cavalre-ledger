//! Solana address adaptation for the shared LedgerLib rules.
use anchor_lang::prelude::Pubkey;
use cavalre_ledger_core::ledger_lib as core;
pub use core::{AccountKind, Error, Store, TokenKind};

pub type Flags = core::Flags<Pubkey>;

/// Derivation neither allocates storage nor registers an account.
pub fn to_address(program: &Pubkey, parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"account", parent.as_ref(), relative.as_ref()], program)
}

pub struct PdaAddresses<'a>(pub &'a Pubkey);

impl core::AddressDerivation<Pubkey> for PdaAddresses<'_> {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        to_address(self.0, parent, relative).0
    }
}

pub use core::{custody, ledger};

/// Resolve through the shared core using this program's PDA derivation.
pub fn effective_flags(
    store: &impl Store<Pubkey>,
    program: &Pubkey,
    ledger_address: &Pubkey,
    parent: &Pubkey,
    relative: &Pubkey,
) -> Result<(Flags, Option<Flags>, Pubkey), Error> {
    core::effective_flags(
        store,
        &PdaAddresses(program),
        ledger_address,
        parent,
        relative,
    )
}
