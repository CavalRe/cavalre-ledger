//! Solana address adaptation for the shared LedgerLib rules.
use anchor_lang::prelude::*;
use cavalre_ledger_core::ledger_lib as core;
pub use core::{AccountKind, Error, Store, TokenKind};

pub type Flags = core::Flags<Pubkey>;
pub type Child = core::Child<Pubkey>;

/// Derivation neither allocates storage nor registers an account.
pub fn to_address(program: &Pubkey, parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[ACCOUNT_NAMESPACE, parent.as_ref(), relative.as_ref()],
        program,
    )
}

/// Full Keccak-256 of the exact name bytes, represented as a 32-byte relative
/// public key. Names require 1–64 UTF-8 bytes, including for named leaves.
/// This is an identifier, not a signer or an allocation. SOURCE uses the same
/// derivation; this helper does not special-case labels.
pub fn name_to_address(name: &str) -> std::result::Result<Pubkey, Error> {
    core::name_to_address(&PdaAddresses(&crate::ID), name)
}

/// Resolve a named child using the existing explicit-address PDA seeds.
pub fn to_address_by_name(
    program: &Pubkey,
    parent: &Pubkey,
    name: &str,
) -> std::result::Result<(Pubkey, u8), Error> {
    Ok(to_address(program, parent, &name_to_address(name)?))
}

pub struct PdaAddresses<'a>(pub &'a Pubkey);

impl core::NameDerivation<Pubkey> for PdaAddresses<'_> {
    fn hash_name(&self, name: &str) -> Pubkey {
        Pubkey::new_from_array(solana_keccak_hasher::hash(name.as_bytes()).to_bytes())
    }
}

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

pub use core::ROOT_NAME;

pub const GLOBAL_ROOT_BUMP: u8 = 250;
pub const GLOBAL_ROOT: Pubkey =
    Pubkey::derive_address_const(&[ROOT_NAME.as_bytes()], Some(GLOBAL_ROOT_BUMP), &crate::ID);

pub const fn global_root_address() -> (Pubkey, u8) {
    (GLOBAL_ROOT, GLOBAL_ROOT_BUMP)
}

/// Reserved Source identity, derived at compile time with no runtime hash cost.
pub const SOURCE: Pubkey =
    Pubkey::new_from_array(keccak_const::Keccak256::new().update(b"Source").finalize());
/// Native SOL asset identity; this System Program address cannot be a token mint.
pub const NATIVE_SOL: Pubkey = Pubkey::new_from_array([0; 32]);
/// Fixed namespace for this account layout. A future layout uses a new namespace.
pub const ACCOUNT_NAMESPACE: &[u8] = b"cavalre.ledger.account";
pub const METADATA_NAMESPACE: &[u8] = b"cavalre.ledger.metadata";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerConfig {
    pub token_kind: u8,
    pub identifier: Pubkey,
    pub authority: Pubkey,
    pub vault: Pubkey,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub parent: Pubkey,
    pub custodian: Pubkey,
    pub kind: u8,
    pub depth: u8,
    pub debit: u128,
    pub credit: u128,
    pub child_index: u32,
    pub ledger: Option<LedgerConfig>,
    /// Relative identities, in insertion order with swap-and-pop removal.
    pub children: Vec<Pubkey>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metadata {
    /// Bump of the associated accounting PDA, not this metadata PDA.
    pub bump: u8,
    pub decimals: u8,
    pub name: String,
    pub symbol: String,
}
impl Record {
    pub fn space(&self) -> usize {
        crate::ledger_storage::children_offset(self.depth) + 4 + 32 * self.children.len()
    }
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut data = vec![0; self.space()];
        crate::ledger_storage::encode(self, &mut data)?;
        Ok(data)
    }
    pub fn flags(&self) -> core::Flags<Pubkey> {
        core::Flags {
            parent: self.parent,
            account_kind: match self.kind {
                0 => core::AccountKind::DebitGroup,
                1 => core::AccountKind::CreditGroup,
                2 => core::AccountKind::DebitLedger,
                _ => core::AccountKind::CreditLedger,
            },
            token_kind: match self.ledger.as_ref().map(|c| c.token_kind) {
                Some(1) => core::TokenKind::Native,
                Some(2) => core::TokenKind::External,
                Some(3) => core::TokenKind::Internal,
                _ => core::TokenKind::Unregistered,
            },
            depth: self.depth,
        }
    }
    pub fn borrowed<'a>(
        &self,
        relative: Pubkey,
        metadata: Option<&'a Metadata>,
    ) -> core::Account<Pubkey, &'a str> {
        core::Account {
            flags: self.flags(),
            relative,
            custodian: self.custodian,
            registered: true,
            implicit_allowed: self.depth > 1,
            children: self.children.len() as u32,
            sub_index: self.child_index,
            balances: core::Balances {
                debit: self.debit,
                credit: self.credit,
            },
            name: metadata.map_or("", |m| m.name.as_str()),
            symbol: metadata.map_or("", |m| m.symbol.as_str()),
            decimals: metadata.map_or(0, |m| m.decimals),
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

/// Internal ledger identifiers are scoped to their authority at creation.
/// Ordinary accounts always use the supplied relative identity unchanged.
pub fn ledger_relative(authority: &Pubkey, identifier: &Pubkey) -> Pubkey {
    if *authority == Pubkey::default() {
        *identifier
    } else {
        Pubkey::new_from_array(
            solana_keccak_hasher::hashv(&[authority.as_ref(), identifier.as_ref()]).to_bytes(),
        )
    }
}
pub fn ledger_pda(authority: &Pubkey, identifier: &Pubkey) -> (Pubkey, u8) {
    to_address(
        &crate::ID,
        &GLOBAL_ROOT,
        &ledger_relative(authority, identifier),
    )
}
pub fn ledger_address(authority: &Pubkey, identifier: &Pubkey) -> Pubkey {
    ledger_pda(authority, identifier).0
}
pub fn metadata_address(parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[METADATA_NAMESPACE, parent.as_ref(), relative.as_ref()],
        &crate::ID,
    )
}
/// Decode a typed account pointer. Identity must be established by the caller
/// through an instruction PDA check or a previously authenticated stored link.
pub fn decode(info: &AccountInfo) -> Result<Record> {
    decode_data(info.key, info.owner, &info.try_borrow_data()?)
}
pub fn decode_data(address: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<Record> {
    require_keys_eq!(*owner, crate::ID, LedgerError::InvalidAccount);
    let record = crate::ledger_storage::decode(data)?;
    if record.depth == 1 {
        require_keys_eq!(*address, GLOBAL_ROOT, LedgerError::InvalidAccount);
        require!(
            record.parent == GLOBAL_ROOT
                && record.custodian == GLOBAL_ROOT
                && record.kind == 0
                && record.debit == 0
                && record.credit == 0,
            LedgerError::InvalidAccount
        );
    } else if record.depth == 2 {
        let config = record.ledger.as_ref().unwrap();
        require!(
            record.parent == GLOBAL_ROOT && record.custodian == GLOBAL_ROOT,
            LedgerError::InvalidAccount
        );
        require_keys_eq!(
            ledger_address(&config.authority, &config.identifier),
            *address,
            LedgerError::InvalidAccount
        );
        require!(
            match config.token_kind {
                1 => config.authority == Pubkey::default() && config.identifier == NATIVE_SOL,
                2 => config.authority == Pubkey::default() && config.identifier != NATIVE_SOL,
                3 => config.authority != Pubkey::default() && config.vault == Pubkey::default(),
                _ => false,
            },
            LedgerError::InvalidAccount
        );
    }
    Ok(record)
}
pub fn decode_metadata_data(
    address: &Pubkey,
    owner: &Pubkey,
    data: &[u8],
    parent: &Pubkey,
    relative: &Pubkey,
) -> Result<Metadata> {
    require_keys_eq!(*owner, crate::ID, LedgerError::InvalidAccount);
    require_keys_eq!(
        *address,
        metadata_address(parent, relative).0,
        LedgerError::InvalidAccount
    );
    let metadata = crate::ledger_storage::decode_metadata(data)?;
    require!(
        metadata.bump == to_address(&crate::ID, parent, relative).1,
        LedgerError::InvalidAccount
    );
    Ok(metadata)
}
