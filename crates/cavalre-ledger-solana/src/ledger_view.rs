//! Read helpers corresponding to LedgerView.sol. Public record data can be read
//! directly by RPC clients; on-program readers validate ownership and identity.
use crate::ledger_lib::{decode, decode_data, to_address, LedgerError, Record};
use anchor_lang::prelude::*;
use cavalre_ledger_core::{ledger_lib as core, ledger_view as view};
use std::collections::BTreeMap;

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

    /// Insert account bytes returned by RPC or read from runtime AccountInfo.
    /// RPC provenance/commitment is the caller's responsibility. The decoder
    /// checks owner, header, length and canonical PDA; it never executes Ledger.
    pub fn insert(&mut self, address: Pubkey, owner: &Pubkey, data: &[u8]) -> Result<()> {
        let record = decode_data(&address, owner, data)?;
        require!(
            !self.records.contains_key(&address),
            LedgerError::InvalidAccount
        );
        self.records.insert(address, Some(record));
        Ok(())
    }

    /// Use only after confirming absence in the same snapshot as other records.
    pub fn insert_missing(&mut self, address: Pubkey) -> Result<()> {
        require!(
            !self.records.contains_key(&address),
            LedgerError::InvalidAccount
        );
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
        to_address(&crate::ID, parent, relative).0
    }
}
impl core::ReadStore<Pubkey> for Reader {
    fn account(
        &self,
        address: &Pubkey,
    ) -> std::result::Result<Option<core::Account<Pubkey>>, core::Error> {
        let record = self
            .records
            .get(address)
            .ok_or(core::Error::MissingAccount)?;
        let Some(record) = record else {
            return Ok(None);
        };
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
        Ok(Some(record.logical()))
    }
}
impl view::ChildIndex<Pubkey> for Reader {
    fn child_addresses(&self, parent: &Pubkey) -> std::result::Result<Vec<Pubkey>, core::Error> {
        Ok(self
            .records
            .iter()
            .filter_map(|(key, record)| {
                record
                    .as_ref()
                    .filter(|r| r.depth > 2 && r.registered && r.parent == *parent)
                    .map(|_| *key)
            })
            .collect())
    }
}
