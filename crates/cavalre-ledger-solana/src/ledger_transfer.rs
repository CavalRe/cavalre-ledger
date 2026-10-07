//! The sole transfer instruction. Use CreateIdempotent first for missing leaves.
//! Account order: signer, ledger, source, destination, then any required custody
//! records, ancestor records and external vault. Metadata is never required.
use crate::{
    ledger_lib::{Child, LedgerError, ACCOUNT_NAMESPACE},
    ledger_storage as storage,
};
use anchor_lang::{
    prelude::Pubkey,
    solana_program::instruction::{AccountMeta, Instruction},
    Discriminator, InstructionData, ToAccountMetas,
};
use cavalre_ledger_core::ledger_lib as core;
use pinocchio::{
    account_info::{AccountInfo, Ref},
    program_error::ProgramError,
    pubkey::pubkey_eq,
};

pub const TRANSFER: &[u8; 8] = b"CVXFER01";
/// Create a default recipient leaf only when the supplied recipient has no data.
/// The no-op does not validate an existing account; transfer performs that check.
/// Include this before `instruction` in the same transaction for first receipt.
/// `remaining` supplies the parent's ancestry and the external vault when needed.
/// The payer funds allocation; the existing custody rules determine spending rights.
pub fn create_idempotent_instruction(
    payer: Pubkey,
    authority: Pubkey,
    ledger: Pubkey,
    child: Child,
    remaining: &[AccountMeta],
) -> Instruction {
    let (address, bump) = crate::ledger_lib::to_address(&crate::ID, &child.parent, &child.relative);
    let mut accounts = crate::accounts::LedgerAccounts {
        payer,
        authority,
        ledger,
        system_program: anchor_lang::system_program::ID,
    }
    .to_account_metas(None);
    // Keep the recipient in a fixed role so the no-op needs only a data-length
    // check. Allocation and transfer authenticate their own account identities.
    accounts.push(AccountMeta::new(address, false));
    // Only creation writes the parent vector. Existing-recipient transfers omit
    // this instruction and never acquire the parent's structural write lock.
    for meta in [AccountMeta::new(child.parent, false)]
        .iter()
        .chain(remaining)
    {
        if let Some(existing) = accounts.iter_mut().find(|a| a.pubkey == meta.pubkey) {
            existing.is_writable |= meta.is_writable;
            existing.is_signer |= meta.is_signer;
        } else {
            accounts.push(meta.clone());
        }
    }
    Instruction {
        program_id: crate::ID,
        accounts,
        data: crate::instruction::CreateIdempotent {
            parent: child.parent,
            relative: child.relative,
            bump,
        }
        .data(),
    }
}
/// Client helper. Bumps are derived off-chain and supplied as instruction data.
pub fn instruction(
    authority: Pubkey,
    ledger: Pubkey,
    from: Child,
    to: Child,
    amount: u128,
    remaining: &[AccountMeta],
) -> Instruction {
    let (from_address, from_bump) =
        crate::ledger_lib::to_address(&crate::ID, &from.parent, &from.relative);
    let (to_address, to_bump) = crate::ledger_lib::to_address(&crate::ID, &to.parent, &to.relative);
    let mut data = Vec::with_capacity(154);
    data.extend_from_slice(TRANSFER);
    for key in [from.parent, from.relative, to.parent, to.relative] {
        data.extend_from_slice(key.as_ref());
    }
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&[from_bump, to_bump]);
    let mut accounts = vec![
        AccountMeta::new_readonly(authority, true),
        AccountMeta::new_readonly(ledger, false),
        AccountMeta::new(from_address, false),
        AccountMeta::new(to_address, false),
    ];
    for meta in remaining {
        if let Some(existing) = accounts.iter_mut().find(|a| a.pubkey == meta.pubkey) {
            existing.is_writable |= meta.is_writable;
            existing.is_signer |= meta.is_signer;
        } else {
            accounts.push(meta.clone());
        }
    }
    Instruction {
        program_id: crate::ID,
        accounts,
        data,
    }
}
#[inline(always)]
fn error(kind: LedgerError) -> ProgramError {
    ProgramError::Custom(kind as u32 + 6000)
}
#[inline(always)]
fn accounting(_: core::Error) -> ProgramError {
    error(LedgerError::Accounting)
}
#[inline(always)]
fn find<'a>(accounts: &'a [AccountInfo], key: &Pubkey) -> Result<&'a AccountInfo, ProgramError> {
    accounts
        .iter()
        .find(|a| pubkey_eq(a.key(), key.as_array()))
        .ok_or(error(LedgerError::MissingAccount))
}
// Keep one checked borrow per validated record during admission. All guards are
// released before the posting phase, including self-transfers and duplicate metas.
struct Record<'a> {
    account: &'a AccountInfo,
    data: Ref<'a, [u8]>,
}
impl<'a> Record<'a> {
    #[inline(always)]
    fn load(account: &'a AccountInfo) -> Result<Self, ProgramError> {
        if !account.is_owned_by(crate::ID.as_array()) {
            return Err(error(LedgerError::InvalidAccount));
        }
        let data = account.try_borrow_data()?;
        if !storage::valid_layout(&data) {
            return Err(error(LedgerError::InvalidAccount));
        }
        Ok(Self { account, data })
    }
    #[inline(always)]
    fn flags(&self) -> core::Flags<Address> {
        core::Flags {
            parent: Address(Pubkey::new_from_array(*storage::key(
                &self.data,
                storage::PARENT,
            ))),
            account_kind: match self.data[storage::KIND] {
                0 => core::AccountKind::DebitGroup,
                1 => core::AccountKind::CreditGroup,
                2 => core::AccountKind::DebitLedger,
                _ => core::AccountKind::CreditLedger,
            },
            token_kind: core::TokenKind::Unregistered,
            depth: self.data[storage::DEPTH],
        }
    }
    #[inline(always)]
    fn resolved(&self) -> core::ResolvedEndpoint<Address> {
        core::ResolvedEndpoint {
            address: Address(Pubkey::new_from_array(*self.account.key())),
            flags: self.flags(),
        }
    }
}
#[inline(always)]
fn flags(account: &AccountInfo) -> Result<core::Flags<Address>, core::Error> {
    Ok(Record::load(account)
        .map_err(|_| core::Error::InvalidAccount)?
        .flags())
}
/// The walk compares PDA keys repeatedly. Keep its platform-independent `Eq`
/// contract while using Pinocchio's word comparison in the Solana adapter.
#[derive(Clone, Copy)]
struct Address(Pubkey);
impl PartialEq for Address {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        pubkey_eq(self.0.as_array(), other.0.as_array())
    }
}
impl Eq for Address {}
struct Store<'a>(&'a [AccountInfo]);
impl core::Store<Address> for Store<'_> {
    #[inline(always)]
    fn flags(&self, key: &Address) -> Result<Option<core::Flags<Address>>, core::Error> {
        flags(find(self.0, &key.0).map_err(|_| core::Error::MissingAccount)?).map(Some)
    }
    fn custody_account(&self, key: &Address) -> Result<Option<Address>, core::Error> {
        let account = find(self.0, &key.0).map_err(|_| core::Error::MissingAccount)?;
        let record = Record::load(account).map_err(|_| core::Error::InvalidAccount)?;
        let data = &record.data;
        Ok(Some(Address(Pubkey::new_from_array(*storage::key(
            data,
            storage::CUSTODIAN,
        )))))
    }
    fn relative(&self, _: &Address) -> Result<Address, core::Error> {
        Err(core::Error::InvalidAccount)
    }
}
impl core::AccountingStore<Address> for Store<'_> {
    fn balances(&self, key: &Address) -> Result<core::Balances, core::Error> {
        let account = find(self.0, &key.0).map_err(|_| core::Error::MissingAccount)?;
        let record = Record::load(account).map_err(|_| core::Error::InvalidAccount)?;
        let data = &record.data;
        Ok(core::Balances {
            debit: storage::balance(data, storage::DEBIT),
            credit: storage::balance(data, storage::CREDIT),
        })
    }
}
#[inline(always)]
fn endpoint<'a>(
    account: &'a AccountInfo,
    identity: &[u8; 64],
    bump: u8,
) -> Result<Record<'a>, ProgramError> {
    if !account.is_owned_by(crate::ID.as_array()) {
        return Err(error(LedgerError::InvalidAccount));
    }
    let data = account.try_borrow_data()?;
    if data.len() != storage::LEAF_LEN
        || !(2..=3).contains(&data[storage::KIND])
        || data[storage::DEPTH] < 3
        || storage::child_index(&data) == 0
        || data[storage::HEADER_LEN..] != [0; 4]
    {
        return Err(error(LedgerError::InvalidAccount));
    }
    let record = Record { account, data };
    // Only this program can create its accounting records, always using the
    // canonical off-curve PDA. Owner + namespace hash binds the supplied bump;
    // an attacker cannot turn an on-curve or alternate-bump address into one.
    // Hash the exact PDA preimage. Parent and relative are already adjacent in
    // instruction data. Grouping slices avoids six seed descriptors and the
    // per-slice SHA syscall minimum; the namespace and address do not change.
    let expected = address(identity, bump);
    if !pubkey_eq(account.key(), &expected)
        || !pubkey_eq(
            storage::key(&record.data, storage::PARENT),
            storage::key(identity, 0),
        )
    {
        return Err(error(LedgerError::InvalidAccount));
    }
    Ok(record)
}
#[inline(always)]
fn address(identity: &[u8; 64], bump: u8) -> [u8; 32] {
    let mut suffix = [0; 54];
    suffix[0] = bump;
    suffix[1..33].copy_from_slice(crate::ID.as_ref());
    suffix[33..].copy_from_slice(b"ProgramDerivedAddress");
    solana_sha256_hasher::hashv(&[ACCOUNT_NAMESPACE, identity, &suffix]).to_bytes()
}
#[inline(always)]
fn custody_index(data: &[u8], ledger: &[u8; 32]) -> Result<u32, ProgramError> {
    if data[storage::DEPTH] != 3 || !pubkey_eq(storage::key(data, storage::PARENT), ledger) {
        return Err(error(LedgerError::InvalidAccount));
    }
    Ok(storage::child_index(data))
}
#[inline(always)]
fn custodian(
    accounts: &[AccountInfo],
    key: &[u8; 32],
    from: &Record<'_>,
    to: &Record<'_>,
    ledger: &[u8; 32],
) -> Result<u32, ProgramError> {
    // Reuse a validated endpoint whenever a stored link points to it. This is
    // record reuse, independent of the accounting path or the endpoint's depth.
    if pubkey_eq(key, from.account.key()) {
        custody_index(&from.data, ledger)
    } else if pubkey_eq(key, to.account.key()) {
        custody_index(&to.data, ledger)
    } else {
        let record = Record::load(find(accounts, &Pubkey::new_from_array(*key))?)?;
        custody_index(&record.data, ledger)
    }
}
pub fn process(accounts: &[AccountInfo], input: &[u8]) -> Result<(), ProgramError> {
    if input.len() != 154 || input.get(..8) != Some(TRANSFER) || accounts.len() < 4 {
        return Err(ProgramError::InvalidInstructionData);
    }
    if !accounts[0].is_signer() {
        return Err(error(LedgerError::Unauthorized));
    }
    let key = |at| storage::key(input, at);
    let amount = u128::from_le_bytes(input[136..152].try_into().unwrap());
    let from = endpoint(&accounts[2], input[8..72].try_into().unwrap(), input[152])?;
    let to = endpoint(&accounts[3], input[72..136].try_into().unwrap(), input[153])?;
    let ledger = Pubkey::new_from_array(*accounts[1].key());
    let from_custodian = storage::key(&from.data, storage::CUSTODIAN);
    let to_custodian = storage::key(&to.data, storage::CUSTODIAN);
    let index = custodian(accounts, from_custodian, &from, &to, ledger.as_array())?;
    if !pubkey_eq(from_custodian, to_custodian) {
        custodian(accounts, to_custodian, &from, &to, ledger.as_array())?;
    }
    let ledger_record = Record::load(&accounts[1])?;
    if ledger_record.data[storage::DEPTH] != 2 {
        return Err(error(LedgerError::InvalidAccount));
    }
    let data = &ledger_record.data;
    let token_kind = data[storage::TOKEN_KIND];
    let expected = if token_kind == 3 {
        *storage::key(data, storage::AUTHORITY)
    } else {
        if from.data[storage::KIND] != 2 || to.data[storage::KIND] != 2 {
            return Err(error(LedgerError::InvalidKind));
        }
        // Layout validation established a nonzero child index.
        let index = (index - 1) as usize;
        let at = storage::LEDGER_HEADER_LEN + 4 + index * 32;
        let relative: [u8; 32] = data
            .get(at..at + 32)
            .ok_or(error(LedgerError::InvalidAccount))?
            .try_into()
            .unwrap();
        // Preserve backing admission on every external-ledger mutation.
        let vault = find(
            accounts,
            &Pubkey::new_from_array(*storage::key(data, storage::VAULT)),
        )?;
        let backing = backing(vault, data, &ledger, token_kind)?;
        if backing < storage::balance(data, storage::CREDIT) {
            return Err(error(LedgerError::Undercollateralized));
        }
        relative
    };
    if !pubkey_eq(accounts[0].key(), &expected) {
        return Err(error(LedgerError::Unauthorized));
    }
    drop(ledger_record);
    if pubkey_eq(accounts[2].key(), accounts[3].key()) {
        // Public debit self-transfers still check available funds.
        if token_kind != 3 && storage::balance(&from.data, storage::DEBIT) < amount {
            return Err(error(LedgerError::Accounting));
        }
        return Ok(());
    }
    if pubkey_eq(key(8), key(72))
        && from.data[storage::DEPTH] == to.data[storage::DEPTH]
        && from.data[storage::KIND] == to.data[storage::KIND]
    {
        let credit = from.data[storage::KIND] == 3;
        let offset = if credit {
            storage::CREDIT
        } else {
            storage::DEBIT
        };
        let (from_after, to_after) = core::sibling_amounts(
            storage::balance(&from.data, offset),
            storage::balance(&to.data, offset),
            credit,
            amount,
        )
        .map_err(accounting)?;
        drop(from);
        drop(to);
        if amount != 0 {
            if !accounts[2].is_writable() || !accounts[3].is_writable() {
                return Err(error(LedgerError::InvalidAccount));
            }
            storage::set_balance(&mut accounts[2].try_borrow_mut_data()?, offset, from_after);
            storage::set_balance(&mut accounts[3].try_borrow_mut_data()?, offset, to_after);
        }
        emit_pair(
            &ledger,
            accounts[2].key(),
            accounts[3].key(),
            amount,
            from_after,
            to_after,
        );
        return Ok(());
    }
    if token_kind != 3 && storage::balance(&from.data, storage::DEBIT) < amount {
        return Err(error(LedgerError::Accounting));
    }
    let resolved_from = from.resolved();
    let resolved_to = to.resolved();
    drop(from);
    drop(to);
    walk(accounts, &ledger, resolved_from, resolved_to, amount)
}

/// Skip allocation whenever the supplied recipient already has data.
/// This no-op does not certify a valid Ledger account. Transfer independently
/// validates its endpoints; allocation validates identity, authority and backing.
/// Account 4 is the recipient, immediately after the four LedgerAccounts roles.
pub fn existing_recipient(accounts: &[AccountInfo], input: &[u8]) -> Result<bool, ProgramError> {
    if input.len() != 73 || accounts.len() < 5 {
        return Err(ProgramError::InvalidInstructionData);
    }
    Ok(!accounts[4].data_is_empty())
}

#[inline(never)]
fn walk(
    accounts: &[AccountInfo],
    ledger: &Pubkey,
    resolved_from: core::ResolvedEndpoint<Address>,
    resolved_to: core::ResolvedEndpoint<Address>,
    amount: u128,
) -> Result<(), ProgramError> {
    let changes = core::transfer_resolved(
        &Store(accounts),
        &Address(*ledger),
        resolved_from,
        resolved_to,
        amount,
    )
    .map_err(accounting)?;
    // Stage arithmetic and permissions before the first write.
    for change in &changes {
        if change.before != change.after && !find(accounts, &change.absolute.0)?.is_writable() {
            return Err(error(LedgerError::InvalidAccount));
        }
    }
    for change in changes {
        if change.before != change.after {
            let mut data = find(accounts, &change.absolute.0)?.try_borrow_mut_data()?;
            storage::set_balance(&mut data, storage::DEBIT, change.after.debit);
            storage::set_balance(&mut data, storage::CREDIT, change.after.credit);
        }
        for (credit, side) in [(true, change.credit), (false, change.debit)] {
            if let Some(side) = side {
                emit(
                    ledger,
                    &change.absolute.0,
                    amount,
                    change.after.get(side),
                    credit,
                );
            }
        }
    }
    Ok(())
}

#[inline(never)]
fn backing(
    vault: &AccountInfo,
    data: &[u8],
    ledger: &Pubkey,
    token_kind: u8,
) -> Result<u128, ProgramError> {
    Ok(if token_kind == 1 {
        if !vault.is_owned_by(&[0; 32]) || !vault.data_is_empty() {
            return Err(error(LedgerError::InvalidAccount));
        }
        use anchor_lang::prelude::SolanaSysvar;
        let reserve = anchor_lang::prelude::Rent::get()
            .map_err(|_| error(LedgerError::InvalidAccount))?
            .minimum_balance(0);
        u128::from(
            vault
                .lamports()
                .checked_sub(reserve)
                .ok_or(error(LedgerError::Undercollateralized))?,
        )
    } else {
        if !vault.is_owned_by(anchor_spl::token::ID.as_array())
            && !vault.is_owned_by(anchor_spl::token_interface::ID.as_array())
        {
            return Err(error(LedgerError::InvalidAccount));
        }
        let token = vault.try_borrow_data()?;
        if token.len() < 165
            || !matches!(token[108], 1 | 2)
            || !pubkey_eq(
                storage::key(&token, 0),
                storage::key(data, storage::IDENTIFIER),
            )
            || !pubkey_eq(storage::key(&token, 32), ledger.as_array())
        {
            return Err(error(LedgerError::InvalidAccount));
        }
        if vault.is_owned_by(anchor_spl::token_interface::ID.as_array()) {
            validate_token2022(&token)?;
        }
        u128::from(u64::from_le_bytes(token[64..72].try_into().unwrap()))
    })
}

#[inline(never)]
fn validate_token2022(token: &[u8]) -> Result<(), ProgramError> {
    if token.len() == 165 {
        // The caller already checked initialized state, mint and authority.
        // A base-only account has no TLV section. Match Account::unpack's
        // remaining validation (the three COption tags) without copying its
        // unused delegate, native-reserve and close-authority payloads.
        for at in [72, 109, 129] {
            if u32::from_le_bytes(token[at..at + 4].try_into().unwrap()) > 1 {
                return Err(error(LedgerError::InvalidAccount));
            }
        }
    } else {
        use spl_token_2022_interface::extension::{
            BaseStateWithExtensions, ExtensionType, StateWithExtensions,
        };
        let state = StateWithExtensions::<spl_token_2022_interface::state::Account>::unpack(token)
            .map_err(|_| error(LedgerError::InvalidAccount))?;
        for extension in state
            .get_extension_types()
            .map_err(|_| error(LedgerError::InvalidAccount))?
        {
            if !matches!(
                extension,
                ExtensionType::ImmutableOwner | ExtensionType::TransferHookAccount
            ) {
                return Err(error(LedgerError::UnsupportedToken));
            }
        }
    }
    Ok(())
}

#[inline(always)]
fn emit(ledger: &Pubkey, account: &Pubkey, amount: u128, balance: u128, credit: bool) {
    let mut event = [0u8; 104];
    event[..8].copy_from_slice(if credit {
        crate::ledger::Credit::DISCRIMINATOR
    } else {
        crate::ledger::Debit::DISCRIMINATOR
    });
    event[8..40].copy_from_slice(ledger.as_ref());
    event[40..72].copy_from_slice(account.as_ref());
    event[72..88].copy_from_slice(&amount.to_le_bytes());
    event[88..].copy_from_slice(&balance.to_le_bytes());
    anchor_lang::solana_program::log::sol_log_data(&[&event]);
}

#[inline(always)]
fn emit_pair(
    ledger: &Pubkey,
    from: &[u8; 32],
    to: &[u8; 32],
    amount: u128,
    from_after: u128,
    to_after: u128,
) {
    let mut event = [0u8; 104];
    event[..8].copy_from_slice(crate::ledger::Credit::DISCRIMINATOR);
    event[8..40].copy_from_slice(ledger.as_ref());
    event[40..72].copy_from_slice(from);
    event[72..88].copy_from_slice(&amount.to_le_bytes());
    event[88..].copy_from_slice(&from_after.to_le_bytes());
    anchor_lang::solana_program::log::sol_log_data(&[&event]);
    event[..8].copy_from_slice(crate::ledger::Debit::DISCRIMINATOR);
    event[40..72].copy_from_slice(to);
    event[88..].copy_from_slice(&to_after.to_le_bytes());
    anchor_lang::solana_program::log::sol_log_data(&[&event]);
}
