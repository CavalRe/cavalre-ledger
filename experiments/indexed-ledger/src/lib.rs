//! Executable indexed-storage experiment. Internal accounting only; no token custody.
use anchor_lang::{prelude::*, system_program};
use cavalre_ledger_core::ledger_lib::{self as ledger_core, AccountingStore, Store};
pub mod storage;
pub use storage::{AccountRef, Header, Record, HEADER_LEN, RECORD_LEN};

pub const ID: Pubkey = Pubkey::new_from_array([42; 32]); // Local simulation only.
pub const INDEX_NAMESPACE: &[u8] = b"cavalre.ledger.index";
pub const METADATA_NAMESPACE: &[u8] = b"cavalre.ledger.metadata";
pub const SOURCE: Pubkey =
    Pubkey::new_from_array(keccak_const::Keccak256::new().update(b"Source").finalize());

#[cfg(target_os = "solana")]
anchor_lang::solana_program::entrypoint!(entry);

#[error_code]
pub enum IndexedError {
    InvalidContainer,
    InvalidRecord,
    Unauthorized,
    MissingAccount,
    Full,
    InvalidIndex,
    Nonempty,
    Accounting,
    DifferentLedger,
    InvalidMetadata,
}
pub(crate) fn err(error: IndexedError) -> anchor_lang::error::Error {
    error.into()
}

/// Structural calls: payer, authority, target container, System, auxiliary PDA,
/// then any other parent/root containers. Transfer: authority, then containers.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub enum Command {
    InitializeLedger,
    InitializeContainer {
        ledger: AccountRef,
    },
    Register {
        parent: AccountRef,
        relative: Pubkey,
        kind: u8,
    },
    Remove {
        index: u32,
        relative: Pubkey,
    },
    Metadata {
        index: u32,
        decimals: u8,
        name: String,
        symbol: String,
    },
    Transfer {
        from: AccountRef,
        to: AccountRef,
        amount: u128,
    },
}

pub fn index_address(container: &Pubkey, parent: AccountRef, relative: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            INDEX_NAMESPACE,
            container.as_ref(),
            parent.container.as_ref(),
            &parent.index.to_le_bytes(),
            relative.as_ref(),
        ],
        &ID,
    )
}
pub fn metadata_address(record: AccountRef) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            METADATA_NAMESPACE,
            record.container.as_ref(),
            &record.index.to_le_bytes(),
        ],
        &ID,
    )
}

pub fn entry<'info>(
    program: &Pubkey,
    accounts: &[AccountInfo<'info>],
    data: &[u8],
) -> anchor_lang::solana_program::entrypoint::ProgramResult {
    if *program != ID {
        return Err(anchor_lang::solana_program::program_error::ProgramError::IncorrectProgramId);
    }
    let command = Command::try_from_slice(data).map_err(|_| {
        anchor_lang::solana_program::program_error::ProgramError::InvalidInstructionData
    })?;
    process(accounts, command).map_err(Into::into)
}
fn signed(account: &AccountInfo<'_>) -> Result<()> {
    require!(account.is_signer, IndexedError::Unauthorized);
    Ok(())
}
fn header(account: &AccountInfo<'_>) -> Result<Header> {
    require_keys_eq!(*account.owner, ID, IndexedError::InvalidContainer);
    Header::read(&account.try_borrow_data()?)
}
fn writable(account: &AccountInfo<'_>) -> Result<()> {
    require!(account.is_writable, IndexedError::InvalidContainer);
    Ok(())
}
fn find<'a, 'info>(
    accounts: &'a [AccountInfo<'info>],
    key: &Pubkey,
) -> Result<&'a AccountInfo<'info>> {
    accounts
        .iter()
        .find(|a| a.key == key)
        .ok_or(err(IndexedError::MissingAccount))
}

/// The shared posting core reads authenticated records from the transaction's
/// supplied containers. A caller cannot substitute an index from another ledger.
struct View<'a, 'info> {
    accounts: &'a [AccountInfo<'info>],
    ledger: AccountRef,
    authority: Pubkey,
}
impl View<'_, '_> {
    fn record(&self, reference: AccountRef) -> Result<Record> {
        let account = find(self.accounts, &reference.container)?;
        let h = header(account)?;
        require!(
            h.ledger == self.ledger && h.authority == self.authority,
            IndexedError::DifferentLedger
        );
        storage::read_record(&account.try_borrow_data()?, reference.index)
    }
}
impl Store<AccountRef> for View<'_, '_> {
    fn flags(
        &self,
        reference: &AccountRef,
    ) -> std::result::Result<Option<ledger_core::Flags<AccountRef>>, ledger_core::Error> {
        self.record(*reference)
            .and_then(Record::flags)
            .map(Some)
            .map_err(|_| ledger_core::Error::InvalidAccount)
    }
    // Internal-ledger authority is authenticated at entry. Do not silently reuse
    // this adapter for public custody: AccountRefs are not signer identities.
    fn custody_account(
        &self,
        _: &AccountRef,
    ) -> std::result::Result<Option<AccountRef>, ledger_core::Error> {
        Err(ledger_core::Error::UnsupportedToken)
    }
    fn relative(&self, _: &AccountRef) -> std::result::Result<AccountRef, ledger_core::Error> {
        Err(ledger_core::Error::UnsupportedToken)
    }
}
impl AccountingStore<AccountRef> for View<'_, '_> {
    fn balances(
        &self,
        reference: &AccountRef,
    ) -> std::result::Result<ledger_core::Balances, ledger_core::Error> {
        self.record(*reference)
            .map(Record::balances)
            .map_err(|_| ledger_core::Error::InvalidAccount)
    }
}

fn process<'info>(accounts: &[AccountInfo<'info>], command: Command) -> Result<()> {
    if let Command::Transfer { from, to, amount } = command {
        return transfer(accounts, from, to, amount);
    }
    require!(accounts.len() >= 5, IndexedError::MissingAccount);
    let [payer, authority, container, system, auxiliary] = [
        &accounts[0],
        &accounts[1],
        &accounts[2],
        &accounts[3],
        &accounts[4],
    ];
    signed(payer)?;
    signed(authority)?;
    writable(container)?;
    require_keys_eq!(
        *system.key,
        system_program::ID,
        IndexedError::InvalidContainer
    );
    require_keys_eq!(*container.owner, ID, IndexedError::InvalidContainer);
    match command {
        Command::InitializeLedger | Command::InitializeContainer { .. } => {
            signed(container)?;
            let self_root = AccountRef::new(*container.key, 0);
            let ledger = match command {
                Command::InitializeContainer { ledger } => {
                    require!(
                        ledger.index == 0 && ledger.container != *container.key,
                        IndexedError::InvalidContainer
                    );
                    let root_container = find(accounts, &ledger.container)?;
                    let h = header(root_container)?;
                    require!(
                        h.ledger == ledger && h.authority == *authority.key,
                        IndexedError::Unauthorized
                    );
                    let root =
                        storage::read_record(&root_container.try_borrow_data()?, ledger.index)?;
                    require!(
                        root.depth == 2 && root.kind == 0,
                        IndexedError::InvalidRecord
                    );
                    ledger
                }
                _ => self_root,
            };
            {
                let mut data = container.try_borrow_mut_data()?;
                require!(
                    data.len() >= HEADER_LEN + 2 * RECORD_LEN
                        && (data.len() - HEADER_LEN).is_multiple_of(RECORD_LEN)
                        && data.iter().all(|b| *b == 0),
                    IndexedError::InvalidContainer
                );
                Header {
                    authority: *authority.key,
                    ledger,
                    len: 0,
                }
                .write(&mut data);
                if ledger == self_root {
                    storage::append(
                        &mut data,
                        Record {
                            custodian: self_root,
                            parent: self_root,
                            depth: 2,
                            kind: 0,
                            debit: 0,
                            credit: 0,
                            child_count: 1,
                        },
                    )?;
                    storage::append(
                        &mut data,
                        Record {
                            custodian: AccountRef::new(*container.key, 1),
                            parent: self_root,
                            depth: 3,
                            kind: 3,
                            debit: 0,
                            credit: 0,
                            child_count: 0,
                        },
                    )?;
                }
            }
            if ledger == self_root {
                create_index(
                    payer,
                    auxiliary,
                    system,
                    *container.key,
                    self_root,
                    SOURCE,
                    1,
                )?;
            }
        }
        Command::Register {
            parent,
            relative,
            kind,
        } => {
            storage::kind(kind)?;
            let h = header(container)?;
            require_keys_eq!(h.authority, *authority.key, IndexedError::Unauthorized);
            let view = View {
                accounts,
                ledger: h.ledger,
                authority: h.authority,
            };
            let p = view.record(parent)?;
            require!(p.kind < 2, IndexedError::InvalidRecord);
            let parent_account = find(accounts, &parent.container)?;
            writable(parent_account)?;
            let depth = p
                .depth
                .checked_add(1)
                .ok_or(err(IndexedError::InvalidRecord))?;
            let count = p
                .child_count
                .checked_add(1)
                .ok_or(err(IndexedError::Full))?;
            let reference = AccountRef::new(*container.key, h.len);
            require!(
                (h.len as usize) < (container.data_len() - HEADER_LEN) / RECORD_LEN,
                IndexedError::Full
            );
            create_index(
                payer,
                auxiliary,
                system,
                *container.key,
                parent,
                relative,
                reference.index,
            )?;
            storage::append(
                &mut container.try_borrow_mut_data()?,
                Record {
                    custodian: if parent == h.ledger {
                        reference
                    } else {
                        p.custodian
                    },
                    parent,
                    depth,
                    kind,
                    debit: 0,
                    credit: 0,
                    child_count: 0,
                },
            )?;
            storage::set_child_count(
                &mut parent_account.try_borrow_mut_data()?,
                parent.index,
                count,
            )?;
            emit_event(b"IDXADD02", &(reference, parent, relative, kind))?;
        }
        Command::Remove { index, relative } => {
            let h = header(container)?;
            require_keys_eq!(h.authority, *authority.key, IndexedError::Unauthorized);
            let reference = AccountRef::new(*container.key, index);
            require!(
                reference != h.ledger && reference != AccountRef::new(h.ledger.container, 1),
                IndexedError::Unauthorized
            );
            let view = View {
                accounts,
                ledger: h.ledger,
                authority: h.authority,
            };
            let record = view.record(reference)?;
            require!(
                record.debit == 0 && record.credit == 0 && record.child_count == 0,
                IndexedError::Nonempty
            );
            validate_index(auxiliary, reference, record.parent, relative)?;
            let parent = view.record(record.parent)?;
            let count = parent
                .child_count
                .checked_sub(1)
                .ok_or(err(IndexedError::InvalidRecord))?;
            let parent_account = find(accounts, &record.parent.container)?;
            writable(parent_account)?;
            storage::set_child_count(
                &mut parent_account.try_borrow_mut_data()?,
                record.parent.index,
                count,
            )?;
            storage::remove(&mut container.try_borrow_mut_data()?, index)?;
            close(auxiliary, payer)?;
            emit_event(b"IDXDEL02", &(reference, record.parent, relative))?;
        }
        Command::Metadata {
            index,
            decimals,
            name,
            symbol,
        } => {
            let h = header(container)?;
            require_keys_eq!(h.authority, *authority.key, IndexedError::Unauthorized);
            storage::read_record(&container.try_borrow_data()?, index)?;
            require!(
                name.len() <= 64 && symbol.len() <= 64,
                IndexedError::InvalidMetadata
            );
            let reference = AccountRef::new(*container.key, index);
            let (expected, bump) = metadata_address(reference);
            require_keys_eq!(*auxiliary.key, expected, IndexedError::InvalidMetadata);
            let encoded = encode(&(decimals, name, symbol))?;
            if auxiliary.owner == &ID {
                require!(
                    auxiliary.try_borrow_data()?.as_ref() == encoded.as_slice(),
                    IndexedError::InvalidMetadata
                );
            } else {
                allocate(
                    payer,
                    auxiliary,
                    system,
                    &[
                        METADATA_NAMESPACE,
                        container.key.as_ref(),
                        &index.to_le_bytes(),
                        &[bump],
                    ],
                    encoded.len(),
                )?;
                auxiliary.try_borrow_mut_data()?.copy_from_slice(&encoded);
            }
        }
        Command::Transfer { .. } => unreachable!(),
    }
    Ok(())
}

fn transfer(
    accounts: &[AccountInfo<'_>],
    from: AccountRef,
    to: AccountRef,
    amount: u128,
) -> Result<()> {
    let authority = accounts.first().ok_or(err(IndexedError::MissingAccount))?;
    signed(authority)?;
    let h = header(find(accounts, &from.container)?)?;
    require_keys_eq!(h.authority, *authority.key, IndexedError::Unauthorized);
    let view = View {
        accounts,
        ledger: h.ledger,
        authority: h.authority,
    };
    let changes = ledger_core::transfer_resolved(
        &view,
        &h.ledger,
        ledger_core::ResolvedEndpoint {
            address: from,
            flags: view.record(from)?.flags()?,
        },
        ledger_core::ResolvedEndpoint {
            address: to,
            flags: view.record(to)?.flags()?,
        },
        amount,
    )
    .map_err(|_| err(IndexedError::Accounting))?;
    // Admission of every write precedes mutation. Solana supplies transaction
    // atomicity, including subsequent instruction failures and event visibility.
    for c in &changes {
        if c.before != c.after {
            writable(find(accounts, &c.absolute.container)?)?;
        }
    }
    for c in changes {
        if c.before != c.after {
            storage::set_balances(
                &mut find(accounts, &c.absolute.container)?.try_borrow_mut_data()?,
                c.absolute.index,
                c.after,
            )?;
        }
        if let Some(side) = c.credit {
            emit_event(
                b"IDXCRD02",
                &(h.ledger, c.absolute, amount, c.after.get(side)),
            )?;
        }
        if let Some(side) = c.debit {
            emit_event(
                b"IDXDBT02",
                &(h.ledger, c.absolute, amount, c.after.get(side)),
            )?;
        }
    }
    Ok(())
}

fn emit_event(tag: &[u8; 8], value: &impl AnchorSerialize) -> Result<()> {
    let bytes = encode(value)?;
    anchor_lang::solana_program::log::sol_log_data(&[tag, &bytes]);
    Ok(())
}
pub fn validate_index(
    account: &AccountInfo<'_>,
    reference: AccountRef,
    parent: AccountRef,
    relative: Pubkey,
) -> Result<()> {
    require_keys_eq!(*account.owner, ID, IndexedError::InvalidIndex);
    require_keys_eq!(
        *account.key,
        index_address(&reference.container, parent, &relative).0,
        IndexedError::InvalidIndex
    );
    require!(
        account.try_borrow_data()?.as_ref() == reference.index.to_le_bytes(),
        IndexedError::InvalidIndex
    );
    Ok(())
}
fn create_index<'info>(
    payer: &AccountInfo<'info>,
    index: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    container: Pubkey,
    parent: AccountRef,
    relative: Pubkey,
    value: u32,
) -> Result<()> {
    let (expected, bump) = index_address(&container, parent, &relative);
    require_keys_eq!(*index.key, expected, IndexedError::InvalidIndex);
    allocate(
        payer,
        index,
        system,
        &[
            INDEX_NAMESPACE,
            container.as_ref(),
            parent.container.as_ref(),
            &parent.index.to_le_bytes(),
            relative.as_ref(),
            &[bump],
        ],
        4,
    )?;
    index
        .try_borrow_mut_data()?
        .copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn allocate<'info>(
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
    size: usize,
) -> Result<()> {
    writable(account)?;
    require!(
        account.data_is_empty() && account.owner == &system_program::ID,
        IndexedError::InvalidIndex
    );
    let minimum = Rent::get()?.minimum_balance(size);
    let signer = &[seeds];
    if account.lamports() == 0 {
        return system_program::create_account(
            CpiContext::new_with_signer(
                *system.key,
                system_program::CreateAccount {
                    from: payer.clone(),
                    to: account.clone(),
                },
                signer,
            ),
            minimum,
            size as u64,
            &ID,
        );
    }
    let top_up = minimum.saturating_sub(account.lamports());
    if top_up > 0 {
        system_program::transfer(
            CpiContext::new(
                *system.key,
                system_program::Transfer {
                    from: payer.clone(),
                    to: account.clone(),
                },
            ),
            top_up,
        )?;
    }
    system_program::allocate(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Allocate {
                account_to_allocate: account.clone(),
            },
            signer,
        ),
        size as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Assign {
                account_to_assign: account.clone(),
            },
            signer,
        ),
        &ID,
    )
}
fn close(account: &AccountInfo<'_>, payer: &AccountInfo<'_>) -> Result<()> {
    writable(account)?;
    writable(payer)?;
    let total = payer
        .lamports()
        .checked_add(account.lamports())
        .ok_or(err(IndexedError::Accounting))?;
    **payer.try_borrow_mut_lamports()? = total;
    **account.try_borrow_mut_lamports()? = 0;
    account.resize(0)?;
    account.assign(&system_program::ID);
    Ok(())
}

pub fn encode(value: &impl AnchorSerialize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    value.serialize(&mut bytes)?;
    Ok(bytes)
}
