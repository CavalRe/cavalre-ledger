//! Solana address adaptation for the shared LedgerLib rules.
use anchor_lang::prelude::*;
use cavalre_ledger_core::ledger_lib as core;
pub use core::{AccountKind, Error, Store, TokenKind};

pub type Flags = core::Flags<Pubkey>;
pub type Child = core::Child<Pubkey>;

/// Logical identity: Keccak-256 of exactly the two packed 32-byte identifiers.
/// No program ID, prefix, bump, curve check, allocation or registration.
pub fn to_address(parent: &Pubkey, relative: &Pubkey) -> Pubkey {
    Pubkey::new_from_array(
        solana_keccak_hasher::hashv(&[parent.as_ref(), relative.as_ref()]).to_bytes(),
    )
}

/// Physical record storage. This is not the logical Ledger address.
pub fn account_storage_address(parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"account", parent.as_ref(), relative.as_ref()],
        &crate::ID,
    )
}

/// Full Keccak-256 of the exact name bytes, represented as a 32-byte identity.
pub fn name_to_address(name: &str) -> std::result::Result<Pubkey, Error> {
    core::name_to_address(&Addresses, name)
}

pub fn to_address_by_name(parent: &Pubkey, name: &str) -> std::result::Result<Pubkey, Error> {
    Ok(to_address(parent, &name_to_address(name)?))
}

pub struct Addresses;
impl core::NameDerivation<Pubkey> for Addresses {
    fn hash_name(&self, name: &str) -> Pubkey {
        Pubkey::new_from_array(solana_keccak_hasher::hash(name.as_bytes()).to_bytes())
    }
}
impl core::AddressDerivation<Pubkey> for Addresses {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        to_address(parent, relative)
    }
}

pub use core::{custody, ledger};

/// Resolve through the shared core using logical Ledger identities.
pub fn effective_flags(
    store: &impl Store<Pubkey>,
    ledger_address: &Pubkey,
    parent: &Pubkey,
    relative: &Pubkey,
) -> std::result::Result<(Flags, Option<Flags>, Pubkey), Error> {
    core::effective_flags(store, &Addresses, ledger_address, parent, relative)
}

pub use core::ROOT_NAME;

/// Canonical bump for this program ID; checked against the SDK derivation in tests.
pub const GLOBAL_ROOT_BUMP: u8 = 250;
/// Fixed Root identity, hashed at compile time rather than during each operation.
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
    pub fn is_container(&self) -> bool {
        self.registered && self.kind < 2
    }
    pub fn address(&self) -> Pubkey {
        if self.depth <= 2 {
            self.root
        } else {
            to_address(&self.parent, &self.relative)
        }
    }
    /// Physical location of this record; stored bump avoids a curve search.
    pub fn storage_address(&self) -> Pubkey {
        if self.depth == 1 {
            GLOBAL_ROOT
        } else if self.depth == 2 {
            Pubkey::derive_address(
                &[b"ledger", self.scope.as_ref(), self.identifier.as_ref()],
                Some(self.bump),
                &crate::ID,
            )
        } else {
            Pubkey::derive_address(
                &[b"account", self.parent.as_ref(), self.relative.as_ref()],
                Some(self.bump),
                &crate::ID,
            )
        }
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
    // Creation establishes a canonical off-curve PDA. Only this program can
    // write its records, and mutations preserve their seeds. Recheck the exact
    // address hash here without repeating the curve check on every read.
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
        data.len() >= SPACE && &data[..8] == MAGIC,
        LedgerError::InvalidAccount
    );
    crate::ledger_storage::capacity(data)?;
    let r =
        Record::deserialize(&mut &data[8..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require!(
        r.kind <= 1
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
    let derive = |seeds: &[&[u8]; 3]| -> Result<Pubkey> {
        if CANONICAL {
            let (key, bump) = Pubkey::find_program_address(seeds, &crate::ID);
            require!(bump == r.bump, LedgerError::InvalidAccount);
            Ok(key)
        } else {
            Ok(Pubkey::derive_address(seeds, Some(r.bump), &crate::ID))
        }
    };
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
        let storage = derive(&[b"ledger", r.scope.as_ref(), r.identifier.as_ref()])?;
        let logical = if r.scope == Pubkey::default() {
            r.identifier
        } else {
            storage
        };
        require_keys_eq!(r.root, logical, LedgerError::InvalidAccount);
        require_keys_eq!(r.relative, r.identifier, LedgerError::InvalidAccount);
        storage
    } else {
        derive(&[b"account", r.parent.as_ref(), r.relative.as_ref()])?
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

/// Read a logical record from its containing group. Group references require
/// their own container; absent leaf entries return None.
pub fn record_at(container: &AccountInfo, address: &Pubkey) -> Result<Option<Record>> {
    let group = decode(container)?;
    if group.address() == *address {
        return Ok(Some(group));
    }
    match crate::ledger_storage::get_with_parent(
        &container.try_borrow_data()?,
        address,
        &group,
        &group.address(),
    )? {
        Some(crate::ledger_storage::Value::Leaf(record)) => {
            require!(
                record.parent == group.address() && record.root == group.root,
                LedgerError::InvalidAccount
            );
            Ok(Some(record))
        }
        Some(crate::ledger_storage::Value::Group(_)) => err!(LedgerError::MissingAccount),
        None => Ok(None),
    }
}
