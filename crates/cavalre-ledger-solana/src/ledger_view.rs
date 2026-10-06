//! Read helpers corresponding to LedgerView.sol. Public record data can be read
//! directly by RPC clients; on-program readers validate ownership and identity.
use crate::ledger_lib::{
    decode, decode_data, global_root_address, root_storage_address, to_address, LedgerError, Record,
};
use crate::ledger_storage as mapping;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use cavalre_ledger_core::{ledger_lib as core, ledger_view as view};
use spl_token_2022_interface::{
    extension::{
        metadata_pointer::MetadataPointer, BaseStateWithExtensions, ExtensionType,
        StateWithExtensions,
    },
    state::Mint,
};
use spl_token_metadata_interface::state::TokenMetadata;
use std::cell::RefCell;
use std::collections::BTreeMap;

pub const METAPLEX_METADATA_PROGRAM: Pubkey =
    pubkey!("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s");

pub fn metadata_address(mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            b"metadata",
            METAPLEX_METADATA_PROGRAM.as_ref(),
            mint.as_ref(),
        ],
        &METAPLEX_METADATA_PROGRAM,
    )
    .0
}

/// Native SOL is accounted for in lamports.
pub const fn native_symbol() -> &'static str {
    "SOL"
}
pub const fn native_decimals() -> u8 {
    9
}

#[derive(Clone)]
struct Labels {
    name: String,
    symbol: String,
}
enum MetadataSource {
    Undefined,
    Inline(Labels),
    Metaplex(Pubkey),
}
struct MintMetadata {
    decimals: u8,
    // Unsupported metadata remains an error until the selected source is resolved.
    labels: std::result::Result<MetadataSource, core::Error>,
}

// Decode only the shared metadata prefix, borrowing name and symbol. URI, creators and arbitrary additional metadata need no heap copies.
// The authenticated owner and source address are checked by callers first.
fn metadata_prefix(data: &[u8]) -> std::result::Result<(Pubkey, &str, &str), core::Error> {
    fn string<'a>(data: &mut &'a [u8]) -> std::result::Result<&'a str, core::Error> {
        let length = data.get(..4).ok_or(core::Error::InvalidMetadata)?;
        let length = u32::from_le_bytes(length.try_into().unwrap()) as usize;
        *data = &data[4..];
        let bytes = data.get(..length).ok_or(core::Error::InvalidMetadata)?;
        *data = &data[length..];
        std::str::from_utf8(bytes).map_err(|_| core::Error::InvalidMetadata)
    }
    let mint = data.get(32..64).ok_or(core::Error::InvalidMetadata)?;
    let mint = Pubkey::new_from_array(mint.try_into().unwrap());
    let mut fields = &data[64..];
    let name = string(&mut fields)?;
    let symbol = string(&mut fields)?;
    Ok((mint, name, symbol))
}

fn mint_metadata(address: &Pubkey, owner: &Pubkey, data: &[u8]) -> Result<MintMetadata> {
    if *owner == spl_token_2022_interface::inline_spl_token::ID {
        let mint = Mint::unpack(data).map_err(|_| error!(LedgerError::InvalidAccount))?;
        return Ok(MintMetadata {
            decimals: mint.decimals,
            labels: Ok(MetadataSource::Metaplex(metadata_address(address))),
        });
    }
    require_keys_eq!(
        *owner,
        spl_token_2022_interface::ID,
        LedgerError::InvalidAccount
    );
    let state = StateWithExtensions::<Mint>::unpack(data)
        .map_err(|_| error!(LedgerError::InvalidAccount))?;
    let extensions = state
        .get_extension_types()
        .map_err(|_| error!(LedgerError::InvalidAccount))?;
    let labels = (|| {
        if !extensions.contains(&ExtensionType::MetadataPointer) {
            return Ok(MetadataSource::Metaplex(metadata_address(address)));
        }
        let pointer = state
            .get_extension::<MetadataPointer>()
            .map_err(|_| core::Error::InvalidMetadata)?;
        let Some(target) = Option::<Pubkey>::from(pointer.metadata_address) else {
            return Ok(MetadataSource::Undefined);
        };
        if target == *address {
            if !extensions.contains(&ExtensionType::TokenMetadata) {
                return Ok(MetadataSource::Undefined);
            }
            let data = state
                .get_extension_bytes::<TokenMetadata>()
                .map_err(|_| core::Error::InvalidMetadata)?;
            let (mint, name, symbol) = metadata_prefix(data)?;
            if mint != *address {
                return Err(core::Error::InvalidMetadata);
            }
            Ok(MetadataSource::Inline(Labels {
                name: name.to_owned(),
                symbol: symbol.to_owned(),
            }))
        } else if target == metadata_address(address) {
            Ok(MetadataSource::Metaplex(target))
        } else {
            Err(core::Error::UnsupportedMetadata)
        }
    })();
    Ok(MintMetadata {
        decimals: state.base.decimals,
        labels,
    })
}

fn metaplex_labels(address: &Pubkey, data: &[u8]) -> Result<Labels> {
    // Metaplex Key::MetadataV1 = 4. Its stable Borsh prefix is update authority,
    // mint, name and symbol. Later optional fields are irrelevant to this query.
    require!(data.first() == Some(&4), LedgerError::InvalidAccount);
    let (mint, name, symbol) =
        metadata_prefix(&data[1..]).map_err(|_| error!(LedgerError::InvalidAccount))?;
    require_keys_eq!(
        *address,
        metadata_address(&mint),
        LedgerError::InvalidAccount
    );
    require!(
        name.len() <= 32 && symbol.len() <= 10,
        LedgerError::InvalidAccount
    );
    Ok(Labels {
        name: name.trim_end_matches('\0').to_owned(),
        symbol: symbol.trim_end_matches('\0').to_owned(),
    })
}

// Registration needs only the mint and its issuer metadata. Share the Reader's
// decoders without constructing its general-purpose account indexes.
#[cfg(feature = "mutations")]
pub(crate) fn registration_metadata(
    mint: &AccountInfo,
    accounts: &[AccountInfo],
) -> Result<(String, String, u8)> {
    let parsed = mint_metadata(mint.key, mint.owner, &mint.try_borrow_data()?)?;
    let canonical = metadata_address(mint.key);
    let labels = if let Some(info) = accounts.iter().find(|a| *a.key == canonical) {
        require_keys_eq!(
            *info.owner,
            METAPLEX_METADATA_PROGRAM,
            LedgerError::InvalidAccount
        );
        Some(metaplex_labels(info.key, &info.try_borrow_data()?)?)
    } else {
        None
    };
    let selected = match parsed
        .labels
        .map_err(|_| error!(LedgerError::InvalidAccount))?
    {
        MetadataSource::Inline(labels) => labels,
        MetadataSource::Metaplex(address) => {
            require_keys_eq!(address, canonical, LedgerError::InvalidAccount);
            labels.ok_or_else(|| error!(LedgerError::MissingAccount))?
        }
        MetadataSource::Undefined => return err!(LedgerError::InvalidAccount),
    };
    Ok((selected.name, selected.symbol, parsed.decimals))
}

pub fn account(info: &AccountInfo) -> Result<Record> {
    decode(info)
}
pub fn debit_balance_of(info: &AccountInfo) -> Result<u128> {
    Ok(decode(info)?.debit)
}
pub fn credit_balance_of(info: &AccountInfo) -> Result<u128> {
    Ok(decode(info)?.credit)
}

/// A read-only, validated set of accounts from one consistent snapshot. This
/// type has no signer, payer, mutation host, token movement or commit capability.
///
/// Unprovided accounts are unknown, not absent. Supply every queried endpoint,
/// parent, root and custody ancestor. Insert confirmed RPC nulls explicitly;
/// runtime readers can supply an empty System-owned account for an absent PDA.
#[derive(Default)]
pub struct Reader {
    // Physical inputs and logical records occupy different address spaces. A
    // token mint and its Ledger record may both be present in the same snapshot.
    // Deduplicate only this supplied input list; queries use the indexes below.
    inputs: Vec<Pubkey>,
    records: BTreeMap<Pubkey, Option<Record>>,
    children: BTreeMap<(Pubkey, u32), Pubkey>,
    locations: BTreeMap<Pubkey, Pubkey>,
    derived: RefCell<BTreeMap<Pubkey, Pubkey>>,
    mints: BTreeMap<Pubkey, MintMetadata>,
    metadata: BTreeMap<Pubkey, Labels>,
}
impl Reader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_account_infos(accounts: &[AccountInfo]) -> Result<Self> {
        let mut reader = Self::new();
        for info in accounts {
            if *info.owner == anchor_lang::system_program::ID && info.data_is_empty() {
                reader.insert_missing(*info.key)?;
            } else {
                reader.insert(*info.key, info.owner, &info.try_borrow_data()?)?;
            }
        }
        Ok(reader)
    }

    /// Insert Ledger, mint or canonical Metaplex account bytes from RPC/runtime.
    /// RPC provenance/commitment is the caller's responsibility. Decoders check
    /// owner, layout and applicable PDA; queries bind the mint to its Ledger root.
    /// Metadata is a snapshot, so use a fresh reader after source updates.
    pub fn insert(&mut self, address: Pubkey, owner: &Pubkey, data: &[u8]) -> Result<()> {
        require!(!self.contains(&address), LedgerError::InvalidAccount);
        if *owner == crate::ID {
            let record = decode_data(&address, owner, data)?;
            if !record.registered && record.depth > 2 {
                self.inputs.push(address);
                return Ok(());
            }
            let logical = record.address();
            require!(
                !self.records.contains_key(&logical),
                LedgerError::InvalidAccount
            );
            self.locations.insert(logical, address);
            for index in 0..record.children {
                let relative = mapping::child(data, index)?.ok_or(LedgerError::InvalidAccount)?;
                self.children.insert((logical, index), relative);
            }
            for index in 0..mapping::len(data) {
                let (key, value) = mapping::entry_with_parent(data, index, &record, &logical)?;
                match value {
                    mapping::Value::Leaf(leaf) => {
                        require!(
                            leaf.parent == logical
                                && leaf.root == record.root
                                && leaf.address() == key,
                            LedgerError::InvalidAccount
                        );
                        require!(
                            !self.records.contains_key(&key),
                            LedgerError::InvalidAccount
                        );
                        self.records.insert(key, Some(leaf));
                        self.locations.insert(key, address);
                    }
                    mapping::Value::Group(storage) => {
                        self.locations.insert(key, storage);
                    }
                }
            }
            self.records.insert(logical, Some(record));
        } else if *owner == METAPLEX_METADATA_PROGRAM {
            let labels = metaplex_labels(&address, data)?;
            self.metadata.insert(address, labels);
        } else {
            let metadata = mint_metadata(&address, owner, data)?;
            self.mints.insert(address, metadata);
        }
        self.inputs.push(address);
        Ok(())
    }

    /// Use only after confirming absence in the same snapshot as other records.
    pub fn insert_missing(&mut self, address: Pubkey) -> Result<()> {
        require!(!self.contains(&address), LedgerError::InvalidAccount);
        require!(
            !self.records.contains_key(&address),
            LedgerError::InvalidAccount
        );
        self.records.insert(address, None);
        self.inputs.push(address);
        Ok(())
    }

    /// Mark a ledger absent only after its physical root storage was confirmed
    /// absent. The mint itself may exist and may be present in this reader.
    pub fn insert_missing_ledger(&mut self, scope: &Pubkey, identifier: &Pubkey) -> Result<()> {
        let storage = root_storage_address(scope, identifier).0;
        let address = if *scope == Pubkey::default() {
            *identifier
        } else {
            storage
        };
        require!(!self.contains(&storage), LedgerError::InvalidAccount);
        require!(
            !self.records.contains_key(&address),
            LedgerError::InvalidAccount
        );
        self.records.insert(address, None);
        self.inputs.push(storage);
        Ok(())
    }

    /// Read issuer metadata before creating an external-token ledger. Pointer
    /// selection and source authentication are shared with all metadata decoding.
    /// Ledger initialization validates and stores the returned snapshot.
    pub fn external_metadata(
        &self,
        mint: &Pubkey,
    ) -> std::result::Result<(String, String, u8), core::Error> {
        let metadata = self.mint(mint)?;
        let labels = match metadata.labels.as_ref().map_err(|e| *e)? {
            MetadataSource::Undefined => return Err(core::Error::InvalidMetadata),
            MetadataSource::Inline(labels) => labels,
            MetadataSource::Metaplex(address) => self.metadata.get(address).ok_or_else(|| {
                if self.contains(address) {
                    core::Error::InvalidMetadata
                } else {
                    core::Error::MissingAccount
                }
            })?,
        };
        Ok((
            labels.name.clone(),
            labels.symbol.clone(),
            metadata.decimals,
        ))
    }
    pub fn account_view(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        relative: &Pubkey,
    ) -> std::result::Result<view::AccountView<Pubkey>, core::Error> {
        view::account(self, ledger, parent, relative)
    }
    /// Exact Ledger record write set for a transfer, using the core posting walk.
    /// Supply a consistent snapshot, including confirmed absent endpoints. Other
    /// required records stay read-only; payer/signature privileges are separate.
    /// This is a client planning helper, not an authorization check.
    pub fn transfer_writable_accounts(
        &self,
        ledger: &Pubkey,
        from: crate::ledger_lib::Child,
        to: crate::ledger_lib::Child,
        amount: u128,
    ) -> std::result::Result<Vec<Pubkey>, core::Error> {
        let mut addresses = view::transfer_writable_accounts(self, ledger, from, to, amount)?;
        for address in &mut addresses {
            if let Some(storage) = self.locations.get(address) {
                *address = *storage;
            } else if let Some(parent) = self.derived.borrow().get(address) {
                *address = *self
                    .locations
                    .get(parent)
                    .ok_or(core::Error::MissingAccount)?;
            } else {
                return Err(core::Error::MissingAccount);
            }
        }
        addresses.sort_unstable();
        addresses.dedup();
        Ok(addresses)
    }
    pub fn name(&self, absolute: &Pubkey) -> std::result::Result<String, core::Error> {
        view::name(self, absolute)
    }
    pub fn symbol(&self, absolute: &Pubkey) -> std::result::Result<Option<String>, core::Error> {
        view::symbol(self, absolute)
    }
    pub fn decimals(&self, absolute: &Pubkey) -> std::result::Result<Option<u8>, core::Error> {
        view::decimals(self, absolute)
    }

    fn contains(&self, address: &Pubkey) -> bool {
        self.inputs.contains(address)
    }

    fn mint(&self, address: &Pubkey) -> std::result::Result<&MintMetadata, core::Error> {
        self.mints.get(address).ok_or_else(|| {
            if self.contains(address) {
                core::Error::InvalidMetadata
            } else {
                core::Error::MissingAccount
            }
        })
    }
    pub fn ledger(&self, absolute: &Pubkey) -> std::result::Result<Option<Pubkey>, core::Error> {
        view::ledger(self, absolute)
    }
    pub fn debit_balance_of(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        relative: &Pubkey,
    ) -> std::result::Result<u128, core::Error> {
        view::debit_balance_of(self, ledger, parent, relative)
    }
    pub fn credit_balance_of(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        relative: &Pubkey,
    ) -> std::result::Result<u128, core::Error> {
        view::credit_balance_of(self, ledger, parent, relative)
    }
    pub fn balance_of(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        relative: &Pubkey,
    ) -> std::result::Result<u128, core::Error> {
        view::balance_of(self, ledger, parent, relative)
    }
    pub fn total_supply(&self, absolute: &Pubkey) -> std::result::Result<u128, core::Error> {
        view::total_supply(self, absolute)
    }
    pub fn sub_accounts(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        start: usize,
        limit: usize,
    ) -> std::result::Result<Vec<Pubkey>, core::Error> {
        view::sub_accounts(self, ledger, parent, start, limit)
    }
    pub fn sub_account_count(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
    ) -> std::result::Result<u32, core::Error> {
        view::sub_account_count(self, ledger, parent)
    }
    pub fn sub_account(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        index: usize,
    ) -> std::result::Result<Pubkey, core::Error> {
        view::sub_account(self, ledger, parent, index)
    }
    pub fn sub_account_index(&self, absolute: &Pubkey) -> std::result::Result<u32, core::Error> {
        view::sub_account_index(self, absolute)
    }
    /// Ledger discovery uses the same count and enumeration as Root's children.
    pub fn ledger_count(&self) -> std::result::Result<u32, core::Error> {
        view::ledger_count(self, &global_root_address().0)
    }
    pub fn ledger_at(&self, index: usize) -> std::result::Result<Pubkey, core::Error> {
        view::ledger_at(self, &global_root_address().0, index)
    }
    pub fn ledgers(
        &self,
        start: usize,
        limit: usize,
    ) -> std::result::Result<Vec<Pubkey>, core::Error> {
        view::ledgers(self, &global_root_address().0, start, limit)
    }
    /// Roots present in this reader, sorted by address. This is deliberately not
    /// ledgerCount: a partial account set cannot establish a global ledger count.
    pub fn known_ledgers(&self) -> std::result::Result<Vec<Pubkey>, core::Error> {
        let mut roots = Vec::new();
        for (address, record) in &self.records {
            if record.as_ref().is_some_and(|r| r.depth == 2) {
                view::total_supply(self, address)?;
                roots.push(*address);
            }
        }
        Ok(roots)
    }
}
impl core::AddressDerivation<Pubkey> for Reader {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        let key = to_address(parent, relative);
        self.derived.borrow_mut().insert(key, *parent);
        key
    }
}
impl core::ReadStore<Pubkey> for Reader {
    fn account(
        &self,
        address: &Pubkey,
    ) -> std::result::Result<Option<core::Account<Pubkey, &str>>, core::Error> {
        let record = match self.records.get(address) {
            Some(record) => record,
            None => {
                // A supplied container proves absence of an entry. A group
                // reference whose container was omitted does not prove absence.
                if !self.locations.contains_key(address) {
                    if let Some(parent) = self.derived.borrow().get(address) {
                        if self
                            .records
                            .get(parent)
                            .is_some_and(|r| r.as_ref().is_some_and(|r| r.kind < 2))
                        {
                            return Ok(None);
                        }
                    }
                }
                return Err(core::Error::MissingAccount);
            }
        };
        let Some(record) = record else {
            return Ok(None);
        };
        if record.depth == 1 {
            // decode_data authenticated Root's fixed identity and fields.
            return Ok(Some(record.borrowed()));
        }
        let root = self
            .records
            .get(&record.root)
            .and_then(|r| r.as_ref())
            .ok_or(core::Error::MissingAccount)?;
        if root.depth != 2 || root.kind != 0 || !root.registered || root.root != record.root {
            return Err(core::Error::InvalidAccount);
        }
        if record.depth == 2 && record.root != *address {
            return Err(core::Error::InvalidAccount);
        }
        Ok(Some(record.borrowed()))
    }
}
impl view::ChildIndex<Pubkey> for Reader {
    fn child_at(&self, parent: &Pubkey, index: u32) -> std::result::Result<Pubkey, core::Error> {
        self.children
            .get(&(*parent, index))
            .copied()
            .ok_or(core::Error::IncompleteIndex)
    }
}
