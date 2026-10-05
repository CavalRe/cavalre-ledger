//! Solana address adaptation for the shared LedgerLib rules.
use anchor_lang::prelude::*;
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
) -> std::result::Result<(Flags, Option<Flags>, Pubkey), Error> {
    core::effective_flags(
        store,
        &PdaAddresses(program),
        ledger_address,
        parent,
        relative,
    )
}

pub const SOURCE: Pubkey = Pubkey::new_from_array([83; 32]);
pub(crate) const MAGIC: &[u8; 8] = b"CVLEDG01";
pub(crate) const SPACE: usize = 512;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub root: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
    pub custodian: Pubkey,
    pub kind: u8,
    pub token_kind: u8,
    pub depth: u8,
    pub registered: bool,
    pub implicit_allowed: bool,
    pub children: u32,
    pub debit: u128,
    pub credit: u128,
    pub name: String,
    // Root-only identity: scope zero for an external mint, authority for internal units.
    pub scope: Pubkey,
    pub identifier: Pubkey,
    pub bump: u8,
}
impl Record {
    pub fn flags(&self) -> core::Flags<Pubkey> {
        core::Flags {
            parent: self.parent,
            account_kind: match self.kind {
                0 => core::AccountKind::DebitGroup,
                1 => core::AccountKind::CreditGroup,
                2 => core::AccountKind::DebitLedger,
                _ => core::AccountKind::CreditLedger,
            },
            token_kind: if self.depth == 2 {
                if self.scope == Pubkey::default() {
                    core::TokenKind::External
                } else {
                    core::TokenKind::Internal
                }
            } else {
                core::TokenKind::Unregistered
            },
            depth: self.depth,
        }
    }
}

#[error_code]
pub enum LedgerError {
    InvalidAccount,
    Unauthorized,
    InvalidKind,
    Nonempty,
    MetadataConflict,
    InvalidName,
    MissingAccount,
    Accounting,
    UnsupportedToken,
    Settlement,
    Undercollateralized,
}

pub fn root_address(scope: &Pubkey, id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"ledger", scope.as_ref(), id.as_ref()], &crate::ID)
}
pub fn decode(info: &AccountInfo) -> Result<Record> {
    decode_data(info.key, info.owner, &info.try_borrow_data()?)
}

/// Decode RPC or runtime account bytes without invoking the mutation program.
pub fn decode_data(address: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<Record> {
    require_keys_eq!(*owner, crate::ID, LedgerError::InvalidAccount);
    require!(
        data.len() == SPACE && &data[..8] == MAGIC,
        LedgerError::InvalidAccount
    );
    let r =
        Record::deserialize(&mut &data[8..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require!(
        r.kind <= 3 && r.depth >= 2 && r.name.len() <= 64,
        LedgerError::InvalidAccount
    );
    let key = if r.depth == 2 {
        root_address(&r.scope, &r.identifier).0
    } else {
        to_address(&crate::ID, &r.parent, &r.relative).0
    };
    require_keys_eq!(key, *address, LedgerError::InvalidAccount);
    Ok(r)
}

impl Record {
    pub fn logical(&self) -> core::Account<Pubkey> {
        self.borrowed().with_name(self.name.clone())
    }
    /// Borrow the name already decoded from storage; field reads allocate nothing.
    pub fn borrowed(&self) -> core::Account<Pubkey, &str> {
        core::Account {
            flags: self.flags(),
            relative: self.relative,
            custodian: self.custodian,
            registered: self.registered,
            implicit_allowed: self.implicit_allowed,
            children: self.children,
            balances: core::Balances {
                debit: self.debit,
                credit: self.credit,
            },
            name: &self.name,
        }
    }
}
