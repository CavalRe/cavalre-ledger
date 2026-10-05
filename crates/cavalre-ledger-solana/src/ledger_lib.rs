//! Solana address adaptation for the shared LedgerLib rules.
use anchor_lang::prelude::*;
use cavalre_ledger_core::ledger_lib as core;
pub use core::{AccountKind, Error, Store, TokenKind};

pub type Flags = core::Flags<Pubkey>;
pub type Child = core::Child<Pubkey>;

/// Derivation neither allocates storage nor registers an account.
pub fn to_address(program: &Pubkey, parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"account", parent.as_ref(), relative.as_ref()], program)
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

pub fn global_root_address() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[ROOT_NAME.as_bytes()], &crate::ID)
}

/// Reserved Source identity, derived at compile time with no runtime hash cost.
pub const SOURCE: Pubkey =
    Pubkey::new_from_array(keccak_const::Keccak256::new().update(b"Source").finalize());
/// Native SOL asset identity; this System Program address cannot be a token mint.
pub const NATIVE_SOL: Pubkey = Pubkey::new_from_array([0; 32]);
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
    /// One-based child position, zero for unregistered leaves and global Root.
    pub sub_index: u32,
    pub symbol: String,
    pub decimals: u8,
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
                    if self.identifier == NATIVE_SOL {
                        core::TokenKind::Native
                    } else {
                        core::TokenKind::External
                    }
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

/// Logical ledger identity. External tokens use the mint itself; native SOL
/// uses NATIVE_SOL. Accounting-only ledgers retain their authority-scoped IDs.
/// An identity does not imply a Solana account, owner or signing capability.
pub fn ledger_address(scope: &Pubkey, id: &Pubkey) -> Pubkey {
    if *scope == Pubkey::default() {
        *id
    } else {
        root_storage_address(scope, id).0
    }
}

/// Physical storage for a ledger root. This PDA is also the token-vault authority;
/// its address is not the external token's logical ledger identity.
pub fn root_storage_address(scope: &Pubkey, id: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"ledger", scope.as_ref(), id.as_ref()], &crate::ID)
}
pub fn decode(info: &AccountInfo) -> Result<Record> {
    // Program-owned records were created with canonical bumps. Authenticate the
    // stored bump in one derivation instead of searching for it again.
    decode_record::<false>(info.key, info.owner, &info.try_borrow_data()?)
}

/// Decode RPC or runtime account bytes without invoking the mutation program.
pub fn decode_data(address: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<Record> {
    decode_record::<true>(address, owner, data)
}

fn decode_record<const CANONICAL: bool>(
    address: &Pubkey,
    owner: &Pubkey,
    data: &[u8],
) -> Result<Record> {
    require_keys_eq!(*owner, crate::ID, LedgerError::InvalidAccount);
    require!(
        data.len() == SPACE && &data[..8] == MAGIC,
        LedgerError::InvalidAccount
    );
    let r =
        Record::deserialize(&mut &data[8..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require!(
        r.kind <= 3
            && r.depth >= 1
            && r.name.len() <= 64
            && r.symbol.len() <= 64
            && (r.depth != 2 || !r.symbol.is_empty())
            && if r.registered && r.depth > 1 {
                r.sub_index > 0
            } else {
                r.sub_index == 0
            },
        LedgerError::InvalidAccount
    );
    let derive = |seeds: &[&[u8]], bumped: &[&[u8]]| -> Result<Pubkey> {
        if CANONICAL {
            let (key, bump) = Pubkey::find_program_address(seeds, &crate::ID);
            require!(bump == r.bump, LedgerError::InvalidAccount);
            Ok(key)
        } else {
            Pubkey::create_program_address(bumped, &crate::ID)
                .map_err(|_| error!(LedgerError::InvalidAccount))
        }
    };
    let bump = [r.bump];
    let key = if r.depth == 1 {
        let (key, canonical_bump) = global_root_address();
        require!(
            r.bump == canonical_bump
                && r.root == key
                && r.parent == key
                && r.relative == key
                && r.custodian == key
                && r.kind == 0
                && r.token_kind == 0
                && r.registered
                && !r.implicit_allowed
                && r.debit == 0
                && r.credit == 0
                && r.name == ROOT_NAME
                && r.scope == Pubkey::default()
                && r.identifier == Pubkey::default(),
            LedgerError::InvalidAccount
        );
        key
    } else if r.depth == 2 {
        require_keys_eq!(
            r.parent,
            global_root_address().0,
            LedgerError::InvalidAccount
        );
        let storage = derive(
            &[b"ledger", r.scope.as_ref(), r.identifier.as_ref()],
            &[b"ledger", r.scope.as_ref(), r.identifier.as_ref(), &bump],
        )?;
        let logical = if r.scope == Pubkey::default() {
            r.identifier
        } else {
            storage
        };
        require_keys_eq!(r.root, logical, LedgerError::InvalidAccount);
        require_keys_eq!(r.relative, r.identifier, LedgerError::InvalidAccount);
        storage
    } else {
        derive(
            &[b"account", r.parent.as_ref(), r.relative.as_ref()],
            &[b"account", r.parent.as_ref(), r.relative.as_ref(), &bump],
        )?
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
            sub_index: self.sub_index,
            balances: core::Balances {
                debit: self.debit,
                credit: self.credit,
            },
            name: &self.name,
            symbol: &self.symbol,
            decimals: self.decimals,
        }
    }
}

/// One ordinary child-array slot. All parents, including global Root, use this
/// layout. A separate PDA per slot permits reads without loading other siblings.
pub const CHILD_SPACE: usize = 80;
pub const CHILD_MAGIC: &[u8; 8] = b"CVCHLD01";
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct ChildSlot {
    pub parent: Pubkey,
    pub index: u32,
    pub relative: Option<Pubkey>,
}
pub fn child_index_address(parent: &Pubkey, index: u32) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"subs", parent.as_ref(), &index.to_le_bytes()],
        &crate::ID,
    )
}
pub fn decode_child_data(address: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<ChildSlot> {
    require_keys_eq!(*owner, crate::ID, LedgerError::InvalidAccount);
    require!(
        data.len() == CHILD_SPACE && data.get(..8) == Some(CHILD_MAGIC),
        LedgerError::InvalidAccount
    );
    let slot =
        ChildSlot::deserialize(&mut &data[8..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require_keys_eq!(
        child_index_address(&slot.parent, slot.index).0,
        *address,
        LedgerError::InvalidAccount
    );
    Ok(slot)
}
