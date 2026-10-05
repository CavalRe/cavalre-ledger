//! Read helpers corresponding to LedgerView.sol. Public record data can be read
//! directly by RPC clients; on-program readers validate ownership and identity.
use crate::ledger_lib::{child_index_address, decode_child_data, ChildSlot, CHILD_MAGIC};
use crate::ledger_lib::{
    decode, decode_data, global_root_address, to_address, LedgerError, Record,
};
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

enum SymbolSource {
    Undefined,
    Inline(String),
    Metaplex(Pubkey),
}
struct MintMetadata {
    decimals: u8,
    // A broken/unsupported symbol must not prevent reading base-unit precision.
    symbol: std::result::Result<SymbolSource, core::Error>,
}

// Decode only the shared metadata prefix, borrowing strings before returning the
// symbol. URI, creators and arbitrary additional metadata need no heap copies.
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
            symbol: Ok(SymbolSource::Metaplex(metadata_address(address))),
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
    let symbol = (|| {
        if !extensions.contains(&ExtensionType::MetadataPointer) {
            return Ok(SymbolSource::Metaplex(metadata_address(address)));
        }
        let pointer = state
            .get_extension::<MetadataPointer>()
            .map_err(|_| core::Error::InvalidMetadata)?;
        let Some(target) = Option::<Pubkey>::from(pointer.metadata_address) else {
            return Ok(SymbolSource::Undefined);
        };
        if target == *address {
            if !extensions.contains(&ExtensionType::TokenMetadata) {
                return Ok(SymbolSource::Undefined);
            }
            let data = state
                .get_extension_bytes::<TokenMetadata>()
                .map_err(|_| core::Error::InvalidMetadata)?;
            let (mint, _, symbol) = metadata_prefix(data)?;
            if mint != *address {
                return Err(core::Error::InvalidMetadata);
            }
            Ok(SymbolSource::Inline(symbol.to_owned()))
        } else if target == metadata_address(address) {
            Ok(SymbolSource::Metaplex(target))
        } else {
            Err(core::Error::UnsupportedMetadata)
        }
    })();
    Ok(MintMetadata {
        decimals: state.base.decimals,
        symbol,
    })
}

fn metaplex_symbol(address: &Pubkey, data: &[u8]) -> Result<String> {
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
    Ok(symbol.trim_end_matches('\0').to_owned())
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
    records: BTreeMap<Pubkey, Option<Record>>,
    children: BTreeMap<Pubkey, ChildSlot>,
    mints: BTreeMap<Pubkey, MintMetadata>,
    symbols: BTreeMap<Pubkey, String>,
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
        if *owner == crate::ID && data.get(..8) == Some(CHILD_MAGIC) {
            let slot = decode_child_data(&address, owner, data)?;
            self.children.insert(address, slot);
        } else if *owner == crate::ID {
            let record = decode_data(&address, owner, data)?;
            self.records.insert(address, Some(record));
        } else if *owner == METAPLEX_METADATA_PROGRAM {
            let symbol = metaplex_symbol(&address, data)?;
            self.symbols.insert(address, symbol);
        } else {
            let metadata = mint_metadata(&address, owner, data)?;
            self.mints.insert(address, metadata);
        }
        Ok(())
    }

    /// Use only after confirming absence in the same snapshot as other records.
    pub fn insert_missing(&mut self, address: Pubkey) -> Result<()> {
        require!(!self.contains(&address), LedgerError::InvalidAccount);
        self.records.insert(address, None);
        Ok(())
    }

    pub fn account_view(
        &self,
        ledger: &Pubkey,
        parent: &Pubkey,
        relative: &Pubkey,
    ) -> std::result::Result<view::AccountView<Pubkey>, core::Error> {
        view::account(self, ledger, parent, relative)
    }
    pub fn name(&self, absolute: &Pubkey) -> std::result::Result<String, core::Error> {
        view::name(self, absolute)
    }
    pub fn symbol(&self, ledger: &Pubkey) -> std::result::Result<Option<String>, core::Error> {
        view::symbol(self, ledger)
    }
    pub fn decimals(&self, ledger: &Pubkey) -> std::result::Result<Option<u8>, core::Error> {
        view::decimals(self, ledger)
    }

    fn contains(&self, address: &Pubkey) -> bool {
        self.records.contains_key(address)
            || self.children.contains_key(address)
            || self.mints.contains_key(address)
            || self.symbols.contains_key(address)
    }

    fn root_record(&self, ledger: &Pubkey) -> std::result::Result<&Record, core::Error> {
        view::total_supply(self, ledger)?;
        self.records
            .get(ledger)
            .and_then(Option::as_ref)
            .ok_or(core::Error::InvalidAccount)
    }

    fn mint(&self, address: &Pubkey) -> std::result::Result<&MintMetadata, core::Error> {
        self.mints.get(address).ok_or(if self.contains(address) {
            core::Error::InvalidMetadata
        } else {
            core::Error::MissingAccount
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
    pub fn total_supply(&self, ledger: &Pubkey) -> std::result::Result<u128, core::Error> {
        view::total_supply(self, ledger)
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
impl view::TokenMetadata<Pubkey> for Reader {
    fn token_symbol(&self, ledger: &Pubkey) -> std::result::Result<Option<String>, core::Error> {
        let root = self.root_record(ledger)?;
        match root.flags().token_kind {
            core::TokenKind::Native => Ok(Some(native_symbol().into())),
            core::TokenKind::Internal => Ok(None),
            core::TokenKind::External => match self
                .mint(&root.identifier)?
                .symbol
                .as_ref()
                .map_err(|e| *e)?
            {
                SymbolSource::Undefined => Ok(None),
                SymbolSource::Inline(symbol) => Ok(Some(symbol.clone())),
                SymbolSource::Metaplex(address) => {
                    if let Some(symbol) = self.symbols.get(address) {
                        Ok(Some(symbol.clone()))
                    } else if matches!(self.records.get(address), Some(None)) {
                        Ok(None)
                    } else if self.contains(address) {
                        Err(core::Error::InvalidMetadata)
                    } else {
                        Err(core::Error::MissingAccount)
                    }
                }
            },
            core::TokenKind::Unregistered => Err(core::Error::InvalidAccount),
        }
    }

    fn token_decimals(&self, ledger: &Pubkey) -> std::result::Result<Option<u8>, core::Error> {
        let root = self.root_record(ledger)?;
        match root.flags().token_kind {
            core::TokenKind::Native => Ok(Some(native_decimals())),
            core::TokenKind::Internal => Ok(None),
            core::TokenKind::External => Ok(Some(self.mint(&root.identifier)?.decimals)),
            core::TokenKind::Unregistered => Err(core::Error::InvalidAccount),
        }
    }
}
impl core::AddressDerivation<Pubkey> for Reader {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        to_address(&crate::ID, parent, relative).0
    }
}
impl core::ReadStore<Pubkey> for Reader {
    fn account(
        &self,
        address: &Pubkey,
    ) -> std::result::Result<Option<core::Account<Pubkey, &str>>, core::Error> {
        let record = self
            .records
            .get(address)
            .ok_or(core::Error::MissingAccount)?;
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
            .get(&child_index_address(parent, index).0)
            .ok_or(core::Error::IncompleteIndex)?
            .relative
            .ok_or(core::Error::InvalidIndex)
    }
}
