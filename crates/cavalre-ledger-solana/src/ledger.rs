//! Solana host for the shared Ledger service. Owns runtime authentication,
//! account encoding/allocation, token calls and transaction rollback integration.
pub use crate::ledger_lib::{decode, ledger_pda, LedgerError, Record, SOURCE};
use crate::ledger_lib::{
    decode_header, global_root_address, ledger_relative, metadata_address, to_address, Header,
    LedgerConfig, Metadata, ACCOUNT_NAMESPACE, METADATA_NAMESPACE, NATIVE_SOL, ROOT_NAME,
};
use anchor_lang::{prelude::*, system_program};
use anchor_spl::token_interface::{
    self as token, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use cavalre_ledger_core::{ledger as service, ledger_lib as core};
use spl_token_2022_interface::extension::{
    BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};
use std::cell::RefCell;

#[derive(Accounts)]
pub struct LedgerAccounts<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    /// CHECK: ledger PDA. load
    /// authenticates ownership and identity. Write access only when changed.
    pub ledger: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    // Remaining: endpoint records, parents, custody ancestors and changed ancestors.
    // New endpoint/Source PDAs must be supplied writable for allocation.
    // External/native ledgers also require their canonical vault, read-only.
}
#[derive(Accounts)]
pub struct RegisterLedger<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    /// CHECK: ledger identity, ownership and data authenticated by load/initialize.
    #[account(mut)]
    pub ledger: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: canonical Root account, decoded or initialized with its first child.
    #[account(mut)]
    pub global_root: UncheckedAccount<'info>,
    // Remaining: writable Source PDA.
}
#[derive(Accounts)]
pub struct RegisterToken<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: derived from the mint, authenticated and initialized in the handler.
    #[account(mut)]
    pub ledger: UncheckedAccount<'info>,
    #[account(mint::token_program=token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    /// CHECK: the current token interface initializes and authenticates the
    /// mint/authority below; the pinned Anchor allocator predates PermissionedBurn.
    #[account(init_if_needed, payer=payer, seeds=[b"vault", ledger.key().as_ref()], bump,
        space=custody_space(&mint.to_account_info(), &token_program.key())?, owner=token_program.key())]
    pub vault: UncheckedAccount<'info>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    /// CHECK: canonical Root account, decoded or initialized with its first child.
    #[account(mut)]
    pub global_root: UncheckedAccount<'info>,
}
#[derive(Accounts)]
pub struct MoveTokens<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    // Deposit payer authority. On withdrawal this signature confers no additional rights.
    pub funding_authority: Signer<'info>,
    /// CHECK: authenticated ledger record and signer seeds in handler. Commit
    /// requires write access when balances change; zero amounts need none.
    pub ledger: UncheckedAccount<'info>,
    #[account(mint::token_program=token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, seeds=[b"vault",ledger.key().as_ref()],bump,
        token::mint=mint,token::authority=ledger,token::token_program=token_program)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint=mint, token::token_program=token_program,
        constraint=wallet.key()!=vault.key() @ LedgerError::InvalidAccount)]
    pub wallet: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RegisterSol<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: canonical native ledger; initialized by the shared Ledger service.
    #[account(mut)]
    pub ledger: UncheckedAccount<'info>,
    #[account(mut, seeds=[b"vault", ledger.key().as_ref()], bump,
        constraint=vault.data_is_empty() @ LedgerError::InvalidAccount)]
    pub vault: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
    /// CHECK: canonical Root account, decoded or initialized with its first child.
    #[account(mut)]
    pub global_root: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct MoveSol<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    // Deposit wallet signer; withdrawal may reuse the branch authority.
    pub funding_authority: Signer<'info>,
    /// CHECK: canonical native ledger; owner and record checked by the host.
    /// Commit requires write access when balances change.
    pub ledger: UncheckedAccount<'info>,
    #[account(mut, seeds=[b"vault", ledger.key().as_ref()], bump,
        constraint=vault.data_is_empty() @ LedgerError::InvalidAccount)]
    pub vault: SystemAccount<'info>,
    /// CHECK: deposit requires this address's signature and System transfer;
    /// withdrawal pays the selected address without requiring its signature.
    #[account(mut, constraint=wallet.key()!=vault.key() @ LedgerError::InvalidAccount)]
    pub wallet: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
}

// Compatibility is determined by token behavior, not by a mint allowlist.
// Inspect extensions on every custody operation, including already-admitted mints.
fn custody_space(mint: &AccountInfo, token_program: &Pubkey) -> Result<usize> {
    use anchor_lang::solana_program::program_pack::Pack;
    if mint.owner != token_program {
        return Err(
            anchor_lang::solana_program::program_error::ProgramError::IncorrectProgramId.into(),
        );
    }
    if mint.owner != &token::ID {
        return Ok(spl_token_2022_interface::state::Account::LEN);
    }
    Ok(
        spl_token_2022_interface::extension::account_len::try_calculate_account_len_from_mint_data(
            &mint.try_borrow_data()?,
            &[],
        )?,
    )
}

fn validate_mint(mint: &AccountInfo) -> Result<Option<Pubkey>> {
    let mut hook = None;
    if mint.owner == &token::ID {
        let data = mint.try_borrow_data()?;
        let state = StateWithExtensions::<spl_token_2022_interface::state::Mint>::unpack(&data)?;
        for extension in state.get_extension_types()? {
            require!(
                matches!(
                    extension,
                    ExtensionType::MintCloseAuthority
                        | ExtensionType::TransferHook
                        | ExtensionType::PermissionedBurn
                        | ExtensionType::MetadataPointer
                        | ExtensionType::TokenMetadata
                        | ExtensionType::GroupPointer
                        | ExtensionType::TokenGroup
                        | ExtensionType::GroupMemberPointer
                        | ExtensionType::TokenGroupMember
                        | ExtensionType::InterestBearingConfig
                        | ExtensionType::ScaledUiAmount
                ),
                LedgerError::UnsupportedToken
            );
            if extension == ExtensionType::TransferHook {
                use spl_token_2022_interface::extension::transfer_hook::TransferHook;
                hook = Option::<Pubkey>::from(state.get_extension::<TransferHook>()?.program_id);
            } else if extension == ExtensionType::PermissionedBurn {
                use spl_token_2022_interface::extension::permissioned_burn::PermissionedBurnConfig;
                state.get_extension::<PermissionedBurnConfig>()?;
            }
        }
    }
    Ok(hook)
}

fn validate_token_account(account: &AccountInfo) -> Result<()> {
    if account.owner == &token::ID {
        let data = account.try_borrow_data()?;
        let state = StateWithExtensions::<spl_token_2022_interface::state::Account>::unpack(&data)?;
        for extension in state.get_extension_types()? {
            require!(
                matches!(
                    extension,
                    ExtensionType::ImmutableOwner | ExtensionType::TransferHookAccount
                ),
                LedgerError::UnsupportedToken
            );
        }
    }
    Ok(())
}

fn initialize_custody(accounts: &RegisterToken) -> Result<u64> {
    use spl_token_2022_interface::extension::StateWithExtensionsMut;
    use spl_token_2022_interface::state::{Account, AccountState};
    let uninitialized = {
        let mut data = accounts.vault.try_borrow_mut_data()?;
        StateWithExtensionsMut::<Account>::unpack_uninitialized(&mut data).is_ok()
    };
    if uninitialized {
        token::initialize_account3(CpiContext::new(
            accounts.token_program.key(),
            token::InitializeAccount3 {
                account: accounts.vault.to_account_info(),
                mint: accounts.mint.to_account_info(),
                authority: accounts.ledger.to_account_info(),
            },
        ))?;
    }
    let data = accounts.vault.try_borrow_data()?;
    let state = StateWithExtensions::<Account>::unpack(&data)?;
    require!(
        state.base.state != AccountState::Uninitialized,
        LedgerError::InvalidAccount
    );
    require_keys_eq!(
        state.base.mint,
        accounts.mint.key(),
        LedgerError::InvalidAccount
    );
    require_keys_eq!(
        state.base.owner,
        accounts.ledger.key(),
        LedgerError::InvalidAccount
    );
    Ok(state.base.amount)
}

// Only hook-enabled transfers take this path. Resolve the hook's declared extra
// records, rather than forwarding arbitrary Ledger accounts or signer privileges.
fn transfer_with_hook<'info>(
    token_program: Pubkey,
    accounts: TransferChecked<'info>,
    remaining: &[AccountInfo<'info>],
    hook: Pubkey,
    amount: u64,
    decimals: u8,
    signer: &[&[&[u8]]],
) -> Result<()> {
    let mut instruction = token::spl_token_2022::instruction::transfer_checked(
        &token_program,
        accounts.from.key,
        accounts.mint.key,
        accounts.to.key,
        accounts.authority.key,
        &[],
        amount,
        decimals,
    )?;
    let mut infos = vec![
        accounts.from.clone(),
        accounts.mint.clone(),
        accounts.to.clone(),
        accounts.authority.clone(),
    ];
    spl_transfer_hook_interface::onchain::add_extra_accounts_for_execute_cpi(
        &mut instruction,
        &mut infos,
        &hook,
        accounts.from,
        accounts.mint,
        accounts.to,
        accounts.authority,
        amount,
        remaining,
    )?;
    anchor_lang::solana_program::program::invoke_signed(&instruction, &infos, signer)?;
    Ok(())
}

struct StoredAccount {
    key: Pubkey,
    record: Header,
    // Only appended/overwritten slots. Untouched children stay in account data.
    child_writes: Vec<(u32, Pubkey)>,
    logical: service::Account<Pubkey>,
    metadata: Option<Metadata>,
    changed: bool,
    metadata_changed: bool,
    new: bool,
    removed: bool,
}
struct DerivedAddress {
    parent: Pubkey,
    relative: Pubkey,
    key: Pubkey,
    bump: u8,
}
struct State {
    records: Vec<StoredAccount>,
}
impl State {
    fn get(&self, key: &Pubkey) -> Result<&Header> {
        self.optional(key)
            .ok_or_else(|| error!(LedgerError::MissingAccount))
    }
    fn optional(&self, key: &Pubkey) -> Option<&Header> {
        self.records
            .iter()
            .find(|a| a.key == *key && !a.removed)
            .map(|a| &a.record)
    }
    fn entry_mut(&mut self, key: &Pubkey) -> Result<&mut StoredAccount> {
        self.records
            .iter_mut()
            .find(|a| a.key == *key)
            .ok_or_else(|| error!(LedgerError::MissingAccount))
    }
}
fn info<'a, 'info>(
    root: &'a AccountInfo<'info>,
    rest: &'a [AccountInfo<'info>],
    key: &Pubkey,
) -> Result<&'a AccountInfo<'info>> {
    if root.key == key {
        return Ok(root);
    }
    rest.iter()
        .find(|i| i.key == key)
        .ok_or_else(|| error!(LedgerError::MissingAccount))
}
fn loaded(key: Pubkey, record: Header, relative: Pubkey) -> StoredAccount {
    let logical = record.borrowed(relative, None).with_name(String::new());
    StoredAccount {
        key,
        record,
        child_writes: Vec::new(),
        logical,
        metadata: None,
        changed: false,
        metadata_changed: false,
        new: false,
        removed: false,
    }
}
fn load<'info>(root: &AccountInfo<'info>, rest: &[AccountInfo<'info>]) -> Result<State> {
    let r = decode_header(root)?;
    require!(r.depth == 2 && r.kind == 0, LedgerError::InvalidAccount);
    let identifier = r.ledger.as_ref().unwrap().identifier;
    // The sBPF bump allocator does not reclaim old Vec buffers. Reserve
    // once, including space for both materialized transfer endpoints.
    let mut records = Vec::with_capacity(rest.len() + 3);
    records.push(loaded(*root.key, r, identifier));
    let mut pending = Vec::with_capacity(rest.len());
    for account in rest {
        if account.owner != &crate::ID {
            continue;
        }
        require!(
            account.key != root.key
                && !pending
                    .iter()
                    .any(|info: &&AccountInfo| info.key == account.key),
            LedgerError::InvalidAccount
        );
        let data = account.try_borrow_data()?;
        if crate::ledger_storage::validate(&data).is_ok() && data[crate::ledger_storage::DEPTH] > 2
        {
            pending.push(account);
        }
    }
    pending.sort_by_key(|info| info.try_borrow_data().unwrap()[crate::ledger_storage::DEPTH]);
    for account in pending {
        let key = *account.key;
        let record = crate::ledger_storage::decode_header(&account.try_borrow_data()?)?;
        let Some(parent) = records.iter().find(|a| a.key == record.parent) else {
            continue;
        };
        let Some(relative) = record.child_index.checked_sub(1).and_then(|i| {
            let parent_info = info(root, rest, &record.parent).ok()?;
            crate::ledger_storage::child_at(&parent_info.try_borrow_data().ok()?, i).ok()
        }) else {
            continue;
        };
        if to_address(&crate::ID, &record.parent, &relative).0 != key {
            continue;
        }
        require!(
            parent.record.kind < 2 && parent.record.depth.checked_add(1) == Some(record.depth),
            LedgerError::InvalidAccount
        );
        require!(
            record.custodian
                == if record.depth == 3 {
                    key
                } else {
                    parent.record.custodian
                },
            LedgerError::InvalidAccount
        );
        records.push(loaded(key, record, relative));
    }
    // Metadata is decoded only at its explicitly derived namespace. A byte
    // pattern alone never turns metadata into an accounting record.
    for entry in &mut records {
        let relative = if let Some(config) = &entry.record.ledger {
            ledger_relative(&config.authority, &config.identifier)
        } else {
            entry.logical.relative
        };
        let key = metadata_address(&entry.record.parent, &relative).0;
        if let Some(account) = rest.iter().find(|a| *a.key == key && a.owner == &crate::ID) {
            let metadata = crate::ledger_lib::decode_metadata_data(
                &key,
                account.owner,
                &account.try_borrow_data()?,
                &entry.record.parent,
                &relative,
            )?;
            entry.logical.name = metadata.name.clone();
            entry.logical.symbol = metadata.symbol.clone();
            entry.logical.decimals = metadata.decimals;
            entry.metadata = Some(metadata);
        }
    }
    for account in rest.iter().filter(|a| a.owner == &crate::ID) {
        let known = records.iter().any(|entry| {
            entry.key == *account.key
                || entry.metadata.as_ref().is_some_and(|_| {
                    let relative = entry
                        .record
                        .ledger
                        .as_ref()
                        .map_or(entry.logical.relative, |c| {
                            ledger_relative(&c.authority, &c.identifier)
                        });
                    metadata_address(&entry.record.parent, &relative).0 == *account.key
                })
        });
        require!(known, LedgerError::InvalidAccount);
    }
    Ok(State { records })
}
fn save(info: &AccountInfo, entry: &StoredAccount) -> Result<()> {
    require!(info.is_writable, LedgerError::InvalidAccount);
    let mut data = info.try_borrow_mut_data()?;
    crate::ledger_storage::encode_header(&entry.record, &mut data)?;
    for (index, relative) in &entry.child_writes {
        crate::ledger_storage::set_child(&mut data, *index, relative)?;
    }
    Ok(())
}
fn resize<'info>(
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    space: usize,
) -> Result<()> {
    if account.data_len() == space {
        return Ok(());
    }
    require!(account.is_writable, LedgerError::InvalidAccount);
    let required = Rent::get()?
        .minimum_balance(space)
        .saturating_sub(account.lamports());
    if required > 0 {
        system_program::transfer(
            CpiContext::new(
                *system.key,
                system_program::Transfer {
                    from: payer.clone(),
                    to: account.clone(),
                },
            ),
            required,
        )?;
    }
    account.resize(space)?;
    Ok(())
}
fn allocate<'info>(
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
    space: usize,
) -> Result<()> {
    require!(
        account.is_writable && account.data_is_empty(),
        LedgerError::InvalidAccount
    );
    require_keys_eq!(
        *account.owner,
        system_program::ID,
        LedgerError::InvalidAccount
    );
    let minimum = Rent::get()?.minimum_balance(space);
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
            space as u64,
            &crate::ID,
        );
    }
    // Anyone may prefund a PDA. Keep the top-up/allocate/assign path for it.
    let required = minimum.saturating_sub(account.lamports());
    if required > 0 {
        system_program::transfer(
            CpiContext::new(
                *system.key,
                system_program::Transfer {
                    from: payer.clone(),
                    to: account.clone(),
                },
            ),
            required,
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
        space as u64,
    )?;
    system_program::assign(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Assign {
                account_to_assign: account.clone(),
            },
            signer,
        ),
        &crate::ID,
    )
}
// All host operations are private to this entry-point adapter. A failed service
// call propagates directly to Anchor; the runtime rolls back the entire transaction.
struct SolanaHost<'a, 'info> {
    root_info: AccountInfo<'info>,
    rest: &'a [AccountInfo<'info>],
    payer: AccountInfo<'info>,
    authority: AccountInfo<'info>,
    system: AccountInfo<'info>,
    global_root_info: Option<AccountInfo<'info>>,
    settlement: Option<Settlement<'a, 'info>>,
    backing: Option<service::Backing<Pubkey>>,
    initialize_vault: Option<Pubkey>,
    root: service::Root<Pubkey>,
    state: State,
    derived: RefCell<Vec<DerivedAddress>>,
}
enum Settlement<'a, 'info> {
    Tokens {
        accounts: &'a mut MoveTokens<'info>,
        hook: Option<Pubkey>,
    },
    Sol {
        accounts: &'a MoveSol<'info>,
        vault_bump: u8,
        rent_reserve: u64,
    },
}
struct HostError(anchor_lang::error::Error);
impl From<anchor_lang::error::Error> for HostError {
    fn from(error: anchor_lang::error::Error) -> Self {
        Self(error)
    }
}
impl From<core::Error> for HostError {
    fn from(error: core::Error) -> Self {
        use core::Error as E;
        Self(match error {
            E::Unauthorized => error!(LedgerError::Unauthorized),
            E::InvalidKind | E::InvalidLedgerAccount => error!(LedgerError::InvalidKind),
            E::NonemptyAccount => error!(LedgerError::Nonempty),
            E::MetadataConflict => error!(LedgerError::MetadataConflict),
            E::InvalidName => error!(LedgerError::InvalidName),
            E::MissingAccount => error!(LedgerError::MissingAccount),
            E::Accounting | E::Overflow | E::InsufficientBalance => error!(LedgerError::Accounting),
            E::UnsupportedToken => error!(LedgerError::UnsupportedToken),
            E::Settlement => error!(LedgerError::Settlement),
            E::Undercollateralized => error!(LedgerError::Undercollateralized),
            _ => error!(LedgerError::InvalidAccount),
        })
    }
}
impl<'a, 'info> SolanaHost<'a, 'info> {
    fn new(
        root_info: AccountInfo<'info>,
        rest: &'a [AccountInfo<'info>],
        payer: AccountInfo<'info>,
        authority: AccountInfo<'info>,
        system: AccountInfo<'info>,
        initialize: Option<(Pubkey, Pubkey)>,
    ) -> Result<Self> {
        let (state, scope, identifier) = if let Some((scope, identifier)) = initialize {
            require_keys_eq!(
                ledger_pda(&scope, &identifier).0,
                *root_info.key,
                LedgerError::InvalidAccount
            );
            (
                if root_info.data_is_empty() {
                    State {
                        records: Vec::with_capacity(rest.len() + 2),
                    }
                } else {
                    load(&root_info, rest)?
                },
                scope,
                identifier,
            )
        } else {
            let state = load(&root_info, rest)?;
            let r = &state.records[0].record;
            let config = r.ledger.as_ref().unwrap();
            let (scope, identifier) = (config.authority, config.identifier);
            (state, scope, identifier)
        };
        let address = *root_info.key;
        let root = service::Root {
            address,
            // load() already authenticated this parent for existing ledgers.
            parent: state
                .optional(&address)
                .map_or_else(|| global_root_address().0, |record| record.parent),
            identifier,
            source: SOURCE,
            authority: (scope != Pubkey::default()).then_some(scope),
        };
        Ok(Self {
            root_info,
            rest,
            payer,
            authority,
            system,
            global_root_info: None,
            settlement: None,
            backing: None,
            initialize_vault: None,
            root,
            state,
            derived: RefCell::new(Vec::new()),
        })
    }
    fn child_address(&self, parent: &Pubkey, relative: &Pubkey) -> (Pubkey, u8) {
        let mut derived = self.derived.borrow_mut();
        if let Some(entry) = derived
            .iter()
            .find(|entry| entry.parent == *parent && entry.relative == *relative)
        {
            return (entry.key, entry.bump);
        }
        let (key, bump) = to_address(&crate::ID, parent, relative);
        derived.push(DerivedAddress {
            parent: *parent,
            relative: *relative,
            key,
            bump,
        });
        (key, bump)
    }
    fn accounts(ctx: &Context<'info, LedgerAccounts<'info>>) -> Result<Self>
    where
        'info: 'a,
    {
        let mut host = Self::new(
            ctx.accounts.ledger.to_account_info(),
            ctx.remaining_accounts,
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.authority.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            None,
        )?;
        if host.root.authority.is_none() {
            let vault_key = host.stored_vault()?;
            let vault = info(&host.root_info, host.rest, &vault_key)?;
            let amount = if host.root.identifier == NATIVE_SOL {
                require_keys_eq!(
                    *vault.owner,
                    system_program::ID,
                    LedgerError::InvalidAccount
                );
                require!(vault.data_is_empty(), LedgerError::InvalidAccount);
                native_backing(vault.lamports(), Rent::get()?.minimum_balance(0))?
            } else {
                require!(
                    vault.owner == &anchor_spl::token::ID || vault.owner == &token::ID,
                    LedgerError::InvalidAccount
                );
                let data = vault.try_borrow_data()?;
                let state =
                    StateWithExtensions::<spl_token_2022_interface::state::Account>::unpack(&data)?;
                require_keys_eq!(
                    state.base.mint,
                    host.root.identifier,
                    LedgerError::InvalidAccount
                );
                require_keys_eq!(
                    state.base.owner,
                    *host.root_info.key,
                    LedgerError::InvalidAccount
                );
                for extension in state.get_extension_types()? {
                    require!(
                        matches!(
                            extension,
                            ExtensionType::ImmutableOwner | ExtensionType::TransferHookAccount
                        ),
                        LedgerError::UnsupportedToken
                    );
                }
                u128::from(state.base.amount)
            };
            host.backing = Some(service::Backing {
                asset: host.root.identifier,
                amount,
            });
        }
        Ok(host)
    }
    fn stored_vault(&self) -> Result<Pubkey> {
        let vault = self
            .state
            .get(&self.root.address)?
            .ledger
            .as_ref()
            .unwrap()
            .vault;
        require!(vault != Pubkey::default(), LedgerError::InvalidAccount);
        Ok(vault)
    }
    fn bind_vault(&mut self, vault: Pubkey) -> Result<()> {
        // Called only with Anchor's canonical, owner/mint/authority-validated
        // vault during registration. Repetition must never replace the binding.
        if self.state.optional(&self.root.address).is_some() {
            require_keys_eq!(self.stored_vault()?, vault, LedgerError::InvalidAccount);
        } else {
            self.initialize_vault = Some(vault);
        }
        Ok(())
    }
    fn with_backing(mut self, asset: Pubkey, amount: u128) -> Self {
        self.backing = Some(service::Backing { asset, amount });
        self
    }
    fn with_global_root(mut self, global: AccountInfo<'info>) -> Result<Self> {
        require_keys_eq!(*global.key, self.root.parent, LedgerError::InvalidAccount);
        require!(global.is_writable, LedgerError::InvalidAccount);
        if !global.data_is_empty() {
            let record = decode_header(&global)?;
            require!(record.depth == 1, LedgerError::InvalidAccount);
            let mut entry = loaded(*global.key, record, *global.key);
            entry.logical.name = ROOT_NAME.into();
            self.state.records.push(entry);
        } else {
            require_keys_eq!(
                *global.owner,
                system_program::ID,
                LedgerError::InvalidAccount
            );
        }
        self.global_root_info = Some(global);
        Ok(self)
    }
    fn account_info(&self, key: &Pubkey) -> Result<&AccountInfo<'info>> {
        if *key == self.root.address {
            return Ok(&self.root_info);
        }
        if let Some(global) = self.global_root_info.as_ref().filter(|a| a.key == key) {
            return Ok(global);
        }
        info(&self.root_info, self.rest, key)
    }
    fn run(mut self, command: service::Command<Pubkey>) -> Result<()> {
        service::execute(&mut self, command).map_err(|error| error.0)
    }
}
impl cavalre_ledger_core::ledger_view::ChildIndex<Pubkey> for SolanaHost<'_, '_> {
    fn child_at(&self, parent: &Pubkey, index: u32) -> std::result::Result<Pubkey, core::Error> {
        let entry = self
            .state
            .records
            .iter()
            .find(|entry| entry.key == *parent && !entry.removed)
            .ok_or(core::Error::MissingAccount)?;
        if index >= entry.record.children {
            return Err(core::Error::InvalidIndex);
        }
        if let Some((_, relative)) = entry.child_writes.iter().find(|(i, _)| *i == index) {
            return Ok(*relative);
        }
        let account = self
            .account_info(parent)
            .map_err(|_| core::Error::MissingAccount)?;
        let data = account
            .try_borrow_data()
            .map_err(|_| core::Error::InvalidAccount)?;
        crate::ledger_storage::child_at(&data, index).map_err(|_| core::Error::InvalidIndex)
    }
}
impl service::Host<Pubkey> for SolanaHost<'_, '_> {
    type Error = HostError;
    fn root(&self) -> service::Root<Pubkey> {
        self.root
    }
    fn backing(&self) -> std::result::Result<service::Backing<Pubkey>, HostError> {
        self.backing
            .ok_or_else(|| core::Error::MissingAccount.into())
    }
    fn authenticate(
        &self,
        role: service::Role,
        _command: &service::Command<Pubkey>,
    ) -> std::result::Result<Pubkey, HostError> {
        let account = match role {
            service::Role::Authority => self.authority.clone(),
            service::Role::TokenPayer => match self.settlement.as_ref() {
                Some(Settlement::Tokens { accounts, .. }) => {
                    accounts.funding_authority.to_account_info()
                }
                Some(Settlement::Sol { accounts, .. }) => {
                    accounts.funding_authority.to_account_info()
                }
                None => return Err(core::Error::Unauthorized.into()),
            },
        };
        if !account.is_signer {
            return Err(core::Error::Unauthorized.into());
        }
        Ok(*account.key)
    }
    fn put(
        &mut self,
        key: Pubkey,
        account: service::Account<Pubkey>,
    ) -> std::result::Result<(), HostError> {
        let is_root = key == self.root.address;
        let is_global = key == self.root.parent;
        let scope = if is_root {
            self.root.authority.unwrap_or_default()
        } else {
            Pubkey::default()
        };
        let identifier = if is_root {
            self.root.identifier
        } else {
            Pubkey::default()
        };
        let (expected, bump) = if is_global {
            global_root_address()
        } else if is_root {
            ledger_pda(&scope, &identifier)
        } else {
            self.child_address(&account.flags.parent, &account.relative)
        };
        if expected != key {
            return Err(core::Error::InvalidAccount.into());
        }
        let children = self.state.optional(&key).map_or(0, |r| r.children);
        let vault = if is_root {
            self.initialize_vault.unwrap_or_else(|| {
                self.state
                    .optional(&key)
                    .and_then(|r| r.ledger.as_ref())
                    .map_or(Pubkey::default(), |c| c.vault)
            })
        } else {
            Pubkey::default()
        };
        let record = Header {
            parent: account.flags.parent,
            custodian: account.custodian,
            kind: match account.flags.account_kind {
                core::AccountKind::DebitGroup => 0,
                core::AccountKind::CreditGroup => 1,
                core::AccountKind::DebitLedger => 2,
                core::AccountKind::CreditLedger => 3,
            },
            depth: account.flags.depth,
            child_index: account.sub_index,
            debit: account.balances.debit,
            credit: account.balances.credit,
            children,
            ledger: is_root.then_some(LedgerConfig {
                token_kind: if self.root.authority.is_some() {
                    3
                } else if identifier == NATIVE_SOL {
                    1
                } else {
                    2
                },
                identifier,
                authority: scope,
                vault,
            }),
        };
        let metadata = (!account.name.is_empty() || !account.symbol.is_empty()).then(|| Metadata {
            bump,
            decimals: account.decimals,
            name: account.name.clone(),
            symbol: account.symbol.clone(),
        });
        if let Some(entry) = self.state.records.iter_mut().find(|a| a.key == key) {
            entry.removed = !entry.new && entry.logical.registered && !account.registered;
            entry.changed |= entry.record != record || entry.removed;
            entry.metadata_changed |= entry.metadata != metadata;
            entry.record = record;
            entry.logical = account;
            entry.metadata = metadata;
        } else {
            self.state.records.push(StoredAccount {
                key,
                record,
                child_writes: Vec::new(),
                logical: account,
                metadata,
                changed: true,
                metadata_changed: true,
                new: true,
                removed: false,
            });
        }
        Ok(())
    }
    fn attach_name(&mut self, key: Pubkey, name: String) -> std::result::Result<(), HostError> {
        let entry = self
            .state
            .records
            .iter()
            .find(|a| a.key == key)
            .ok_or(core::Error::MissingAccount)?;
        if entry.metadata.is_some() || !entry.logical.name.is_empty() {
            return Err(core::Error::MetadataConflict.into());
        }
        let parent = entry.record.parent;
        let relative = entry.logical.relative;
        let metadata_key = metadata_address(&parent, &relative).0;
        let target = self.account_info(&metadata_key)?;
        if !target.is_writable {
            return Err(core::Error::InvalidAccount.into());
        }
        let bump = self.child_address(&parent, &relative).1;
        let entry = self.state.entry_mut(&key)?;
        entry.metadata = Some(Metadata {
            bump,
            name: name.clone(),
            symbol: String::new(),
            decimals: 0,
        });
        entry.logical.name = name;
        entry.metadata_changed = true;
        Ok(())
    }
    fn set_balances(
        &mut self,
        key: Pubkey,
        balances: core::Balances,
    ) -> std::result::Result<(), HostError> {
        let entry = self.state.entry_mut(&key)?;
        entry.changed |=
            entry.record.debit != balances.debit || entry.record.credit != balances.credit;
        entry.record.debit = balances.debit;
        entry.record.credit = balances.credit;
        entry.logical.balances = balances;
        Ok(())
    }
    fn set_children(&mut self, key: Pubkey, children: u32) -> std::result::Result<(), HostError> {
        let entry = self.state.entry_mut(&key)?;
        if entry.record.children != children {
            return Err(core::Error::InvalidIndex.into());
        }
        entry.logical.children = children;
        Ok(())
    }
    fn set_sub_index(&mut self, key: Pubkey, index: u32) -> std::result::Result<(), HostError> {
        let entry = self.state.entry_mut(&key)?;
        entry.changed |= entry.record.child_index != index;
        entry.record.child_index = index;
        entry.logical.sub_index = index;
        Ok(())
    }
    fn set_child(
        &mut self,
        parent: Pubkey,
        index: u32,
        relative: Option<Pubkey>,
    ) -> std::result::Result<(), HostError> {
        let relative = if parent == self.root.parent {
            relative.map(|_| {
                ledger_relative(
                    &self.root.authority.unwrap_or_default(),
                    &self.root.identifier,
                )
            })
        } else {
            relative
        };
        let entry = self.state.entry_mut(&parent)?;
        let children = &mut entry.record.children;
        match relative {
            Some(relative) if index <= *children => {
                if index == *children {
                    *children = children.checked_add(1).ok_or(core::Error::Overflow)?;
                }
                if let Some((_, value)) = entry.child_writes.iter_mut().find(|(i, _)| *i == index) {
                    *value = relative;
                } else {
                    entry.child_writes.push((index, relative));
                }
            }
            None if index.checked_add(1) == Some(*children) => {
                *children -= 1;
                entry.child_writes.retain(|(i, _)| *i != index);
            }
            _ => return Err(core::Error::InvalidIndex.into()),
        }
        entry.changed = true;
        entry.logical.children = *children;
        Ok(())
    }
    fn token_balances(&mut self) -> std::result::Result<service::TokenBalances<Pubkey>, HostError> {
        match self
            .settlement
            .as_mut()
            .ok_or(core::Error::UnsupportedToken)?
        {
            Settlement::Tokens {
                accounts: native, ..
            } => Ok(service::TokenBalances {
                asset: native.mint.key(),
                owner: native.wallet.owner,
                vault: u128::from(native.vault.amount),
                wallet: u128::from(native.wallet.amount),
            }),
            Settlement::Sol {
                accounts,
                rent_reserve,
                ..
            } => Ok(service::TokenBalances {
                asset: NATIVE_SOL,
                owner: accounts.wallet.key(),
                vault: u128::from(
                    accounts
                        .vault
                        .lamports()
                        .checked_sub(*rent_reserve)
                        .ok_or(core::Error::Undercollateralized)?,
                ),
                wallet: u128::from(accounts.wallet.lamports()),
            }),
        }
    }
    fn move_tokens(&mut self, deposit: bool, amount: u128) -> std::result::Result<(), HostError> {
        let settlement = self
            .settlement
            .as_mut()
            .ok_or(core::Error::UnsupportedToken)?;
        let (native, hook) = match settlement {
            Settlement::Tokens { accounts, hook } => (accounts, *hook),
            Settlement::Sol {
                accounts,
                vault_bump,
                ..
            } => {
                let bump = [*vault_bump];
                let seeds: &[&[u8]] = &[b"vault", self.root_info.key.as_ref(), &bump];
                let signer = &[seeds];
                let transfer = system_program::Transfer {
                    from: if deposit {
                        accounts.wallet.to_account_info()
                    } else {
                        accounts.vault.to_account_info()
                    },
                    to: if deposit {
                        accounts.vault.to_account_info()
                    } else {
                        accounts.wallet.to_account_info()
                    },
                };
                let cpi = CpiContext::new(accounts.system_program.key(), transfer);
                system_program::transfer(
                    if deposit {
                        cpi
                    } else {
                        cpi.with_signer(signer)
                    },
                    u64::try_from(amount).map_err(|_| core::Error::Overflow)?,
                )?;
                return Ok(());
            }
        };
        let config = self.state.get(&self.root.address)?.ledger.as_ref().unwrap();
        let relative = ledger_relative(&config.authority, &config.identifier);
        let bump = [ledger_pda(&config.authority, &config.identifier).1];
        let parent = global_root_address().0;
        let seeds: &[&[u8]] = &[ACCOUNT_NAMESPACE, parent.as_ref(), relative.as_ref(), &bump];
        let signer = &[seeds];
        let accounts = TransferChecked {
            from: if deposit {
                native.wallet.to_account_info()
            } else {
                native.vault.to_account_info()
            },
            mint: native.mint.to_account_info(),
            to: if deposit {
                native.vault.to_account_info()
            } else {
                native.wallet.to_account_info()
            },
            authority: if deposit {
                native.funding_authority.to_account_info()
            } else {
                self.root_info.clone()
            },
        };
        if let Some(hook) = hook {
            transfer_with_hook(
                native.token_program.key(),
                accounts,
                self.rest,
                hook,
                u64::try_from(amount).map_err(|_| core::Error::Overflow)?,
                native.mint.decimals,
                if deposit { &[] } else { signer },
            )?;
        } else {
            let cpi = CpiContext::new(native.token_program.key(), accounts);
            token::transfer_checked(
                if deposit {
                    cpi
                } else {
                    cpi.with_signer(signer)
                },
                u64::try_from(amount).map_err(|_| core::Error::Overflow)?,
                native.mint.decimals,
            )?;
        }
        // Anchor decoded the initial balances. Refresh only after the CPI so
        // the core still observes actual token-program settlement on both sides.
        native.vault.reload()?;
        native.wallet.reload()?;
        Ok(())
    }
    fn commit(&mut self) -> std::result::Result<(), HostError> {
        // Materialization is registration on Solana. Keep this platform adaptation
        // outside the shared Solidity-compatible logical lifecycle.
        let implicit: Vec<_> = self
            .state
            .records
            .iter()
            .filter(|e| e.new && !e.logical.registered)
            .map(|e| (e.key, e.record.parent, e.logical.relative))
            .collect();
        for (key, parent, relative) in implicit {
            let index = self.state.get(&parent)?.children;
            self.set_child(parent, index, Some(relative))?;
            let entry = self.state.entry_mut(&key)?;
            entry.record.child_index = index + 1;
            entry.logical.sub_index = index + 1;
            entry.logical.registered = true;
        }
        for entry in &self.state.records {
            let target = self.account_info(&entry.key)?;
            let relative = entry
                .record
                .ledger
                .as_ref()
                .map_or(entry.logical.relative, |c| {
                    ledger_relative(&c.authority, &c.identifier)
                });
            if entry.removed {
                close(target, &self.payer)?;
                let metadata_key = metadata_address(&entry.record.parent, &relative).0;
                // Require the optional metadata address even when it is empty,
                // so closing/recreating a child cannot retain stale metadata.
                let metadata = self.account_info(&metadata_key)?;
                if metadata.owner == &crate::ID {
                    close(metadata, &self.payer)?;
                } else if metadata.owner != &system_program::ID || !metadata.data_is_empty() {
                    return Err(core::Error::InvalidAccount.into());
                }
                continue;
            }
            if entry.new {
                let bump = [if entry.record.depth == 1 {
                    global_root_address().1
                } else {
                    to_address(&crate::ID, &entry.record.parent, &relative).1
                }];
                let seeds: &[&[u8]] = if entry.record.depth == 1 {
                    &[ROOT_NAME.as_bytes(), &bump]
                } else {
                    &[
                        ACCOUNT_NAMESPACE,
                        entry.record.parent.as_ref(),
                        relative.as_ref(),
                        &bump,
                    ]
                };
                allocate(
                    &self.payer,
                    target,
                    &self.system,
                    seeds,
                    entry.record.space(),
                )?;
            } else if entry.changed {
                resize(&self.payer, target, &self.system, entry.record.space())?;
            }
            if entry.changed {
                save(target, entry)?;
            }
            if entry.metadata_changed && entry.record.depth > 1 {
                let (metadata_key, metadata_bump) =
                    metadata_address(&entry.record.parent, &relative);
                // Metadata is optional: create it only when the caller supplies
                // its PDA. Ordinary transfers never require this account.
                if let (Some(metadata), Ok(target)) =
                    (&entry.metadata, self.account_info(&metadata_key))
                {
                    let bytes = crate::ledger_storage::encode_metadata(metadata)?;
                    if target.data_is_empty() {
                        let bump = [metadata_bump];
                        allocate(
                            &self.payer,
                            target,
                            &self.system,
                            &[
                                METADATA_NAMESPACE,
                                entry.record.parent.as_ref(),
                                relative.as_ref(),
                                &bump,
                            ],
                            bytes.len(),
                        )?;
                    } else {
                        if *target.owner != crate::ID {
                            return Err(core::Error::InvalidAccount.into());
                        }
                        resize(&self.payer, target, &self.system, bytes.len())?;
                    }
                    if !target.is_writable {
                        return Err(core::Error::InvalidAccount.into());
                    }
                    target
                        .try_borrow_mut_data()
                        .map_err(anchor_lang::error::Error::from)?
                        .copy_from_slice(&bytes);
                }
            }
        }
        Ok(())
    }
    fn emit(&mut self, event: service::Event<Pubkey>) -> std::result::Result<(), HostError> {
        use service::Event;
        match event {
            Event::LedgerAdded {
                ledger,
                authority,
                identifier,
                name,
                symbol,
                decimals,
            } => emit!(LedgerAdded {
                ledger,
                scope: authority.unwrap_or_default(),
                identifier,
                name,
                symbol,
                decimals
            }),
            Event::SubAccountAdded {
                ledger,
                parent,
                relative,
                is_credit,
            } => emit!(SubAccountAdded {
                ledger,
                parent,
                relative,
                is_credit
            }),
            Event::SubAccountGroupAdded {
                ledger,
                parent,
                relative,
                name,
                is_credit,
            } => emit!(SubAccountGroupAdded {
                ledger,
                parent,
                relative,
                name,
                is_credit
            }),
            Event::SubAccountRemoved {
                ledger,
                parent,
                relative,
            } => emit!(SubAccountRemoved {
                ledger,
                parent,
                relative
            }),
            Event::SubAccountGroupRemoved {
                ledger,
                parent,
                relative,
            } => emit!(SubAccountGroupRemoved {
                ledger,
                parent,
                relative
            }),
            Event::Credit {
                ledger,
                account,
                amount,
                balance,
            } => emit!(Credit {
                ledger,
                account,
                amount,
                balance
            }),
            Event::Debit {
                ledger,
                account,
                amount,
                balance,
            } => emit!(Debit {
                ledger,
                account,
                amount,
                balance
            }),
        }
        Ok(())
    }
    fn atomic(
        &mut self,
        operation: impl FnOnce(&mut Self) -> std::result::Result<(), HostError>,
    ) -> std::result::Result<(), HostError> {
        // run() consumes this host and propagates every failure to the entry point.
        // Solana owns rollback of allocation, token CPIs, and account writes.
        operation(self)
    }
}

pub fn add_ledger<'info>(
    ctx: &Context<'info, RegisterLedger<'info>>,
    id: Pubkey,
    name: String,
    symbol: String,
    decimals: u8,
) -> Result<()> {
    SolanaHost::new(
        ctx.accounts.ledger.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        Some((ctx.accounts.authority.key(), id)),
    )?
    .with_global_root(ctx.accounts.global_root.to_account_info())?
    .run(service::Command::Initialize {
        name,
        symbol,
        decimals,
    })
}
pub fn add_external_token<'info>(ctx: Context<'info, RegisterToken<'info>>) -> Result<()> {
    validate_mint(&ctx.accounts.mint.to_account_info())?;
    let balance = initialize_custody(ctx.accounts)?;
    validate_token_account(&ctx.accounts.vault.to_account_info())?;
    let (name, symbol, decimals) = crate::ledger_view::registration_metadata(
        &ctx.accounts.mint.to_account_info(),
        ctx.remaining_accounts,
    )?;
    let mut host = SolanaHost::new(
        ctx.accounts.ledger.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        Some((Pubkey::default(), ctx.accounts.mint.key())),
    )?
    .with_backing(ctx.accounts.mint.key(), u128::from(balance))
    .with_global_root(ctx.accounts.global_root.to_account_info())?;
    host.bind_vault(ctx.accounts.vault.key())?;
    host.run(service::Command::Initialize {
        name,
        symbol,
        decimals,
    })
}
fn native_backing(lamports: u64, rent_reserve: u64) -> Result<u128> {
    lamports
        .checked_sub(rent_reserve)
        .map(u128::from)
        .ok_or_else(|| error!(LedgerError::Undercollateralized))
}

pub fn add_native_sol<'info>(ctx: Context<'info, RegisterSol<'info>>) -> Result<()> {
    let reserve = Rent::get()?.minimum_balance(0);
    // Repeated initialization is subject to the same backing rule; it must
    // not silently repair an existing vault's rent reserve before checking.
    if !ctx.accounts.ledger.data_is_empty() {
        native_backing(ctx.accounts.vault.lamports(), reserve)?;
    }
    let required = reserve.saturating_sub(ctx.accounts.vault.lamports());
    if required > 0 {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.payer.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                },
            ),
            required,
        )?;
    }
    let mut host = SolanaHost::new(
        ctx.accounts.ledger.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        Some((Pubkey::default(), NATIVE_SOL)),
    )?
    .with_backing(
        NATIVE_SOL,
        native_backing(ctx.accounts.vault.lamports(), reserve)?,
    )
    .with_global_root(ctx.accounts.global_root.to_account_info())?;
    host.bind_vault(ctx.accounts.vault.key())?;
    host.run(service::Command::Initialize {
        name: "SOL".into(),
        symbol: "SOL".into(),
        decimals: 9,
    })
}
pub fn add_account<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    name: String,
    credit: bool,
    group: bool,
) -> Result<()> {
    let kind = match (credit, group) {
        (false, true) => core::AccountKind::DebitGroup,
        (true, true) => core::AccountKind::CreditGroup,
        (false, false) => core::AccountKind::DebitLedger,
        (true, false) => core::AccountKind::CreditLedger,
    };
    SolanaHost::accounts(ctx)?.run(service::Command::Add {
        child: service::Child { parent, relative },
        name,
        kind,
        implicit_allowed: true,
    })
}
pub fn remove_account<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    group: bool,
) -> Result<()> {
    SolanaHost::accounts(ctx)?.run(service::Command::Remove {
        child: service::Child { parent, relative },
        group,
    })
}
pub fn create_idempotent<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    bump: u8,
) -> Result<()> {
    let (address, canonical_bump) = to_address(&crate::ID, &parent, &relative);
    require!(canonical_bump == bump, LedgerError::InvalidAccount);
    let recipient = ctx
        .remaining_accounts
        .first()
        .ok_or(LedgerError::MissingAccount)?;
    require_keys_eq!(*recipient.key, address, LedgerError::InvalidAccount);
    SolanaHost::accounts(ctx)?.run(service::Command::CreateIdempotent {
        child: service::Child { parent, relative },
    })
}
pub fn move_tokens<'info>(
    ctx: Context<'info, MoveTokens<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
    deposit: bool,
) -> Result<()> {
    let hook = validate_mint(&ctx.accounts.mint.to_account_info())?;
    validate_token_account(&ctx.accounts.vault.to_account_info())?;
    validate_token_account(&ctx.accounts.wallet.to_account_info())?;
    let mut host = SolanaHost::new(
        ctx.accounts.ledger.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        None,
    )?;
    host.backing = Some(service::Backing {
        asset: ctx.accounts.mint.key(),
        amount: u128::from(ctx.accounts.vault.amount),
    });
    host.settlement = Some(Settlement::Tokens {
        accounts: ctx.accounts,
        hook,
    });
    host.run(service::Command::MoveTokens {
        child: service::Child { parent, relative },
        amount: u128::from(amount),
        deposit,
    })
}

pub fn move_sol<'info>(
    ctx: Context<'info, MoveSol<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
    deposit: bool,
) -> Result<()> {
    let mut host = SolanaHost::new(
        ctx.accounts.ledger.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.authority.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        None,
    )?;
    let rent_reserve = Rent::get()?.minimum_balance(0);
    host.backing = Some(service::Backing {
        asset: NATIVE_SOL,
        amount: native_backing(ctx.accounts.vault.lamports(), rent_reserve)?,
    });
    host.settlement = Some(Settlement::Sol {
        accounts: ctx.accounts,
        vault_bump: ctx.bumps.vault,
        rent_reserve,
    });
    host.run(service::Command::MoveTokens {
        child: service::Child { parent, relative },
        amount: u128::from(amount),
        deposit,
    })
}

/// Solana encodings of the shared semantic events; see docs/EVENTS.md.
#[event]
pub struct LedgerAdded {
    pub ledger: Pubkey,
    pub scope: Pubkey,
    pub identifier: Pubkey,
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
}
#[event]
pub struct SubAccountAdded {
    pub ledger: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
    pub is_credit: bool,
}
#[event]
pub struct SubAccountGroupAdded {
    pub ledger: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
    pub name: String,
    pub is_credit: bool,
}
#[event]
pub struct SubAccountRemoved {
    pub ledger: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
}
#[event]
pub struct SubAccountGroupRemoved {
    pub ledger: Pubkey,
    pub parent: Pubkey,
    pub relative: Pubkey,
}
#[event]
pub struct Credit {
    pub ledger: Pubkey,
    pub account: Pubkey,
    pub amount: u128,
    pub balance: u128,
}
#[event]
pub struct Debit {
    pub ledger: Pubkey,
    pub account: Pubkey,
    pub amount: u128,
    pub balance: u128,
}

impl core::AddressDerivation<Pubkey> for SolanaHost<'_, '_> {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        self.child_address(parent, relative).0
    }
}

impl core::ReadStore<Pubkey> for SolanaHost<'_, '_> {
    fn account(
        &self,
        key: &Pubkey,
    ) -> std::result::Result<Option<service::Account<Pubkey, &str>>, core::Error> {
        if let Some(entry) = self
            .state
            .records
            .iter()
            .find(|a| a.key == *key && !a.removed)
        {
            return Ok(Some(entry.logical.as_ref()));
        }
        let account = self
            .account_info(key)
            .map_err(|_| core::Error::MissingAccount)?;
        if *account.owner == system_program::ID && account.data_is_empty() {
            Ok(None)
        } else {
            Err(core::Error::InvalidAccount)
        }
    }
}
fn close(account: &AccountInfo, recipient: &AccountInfo) -> Result<()> {
    require!(
        account.is_writable && recipient.is_writable && account.key != recipient.key,
        LedgerError::InvalidAccount
    );
    let balance = recipient
        .lamports()
        .checked_add(account.lamports())
        .ok_or_else(|| error!(LedgerError::Accounting))?;
    **recipient.try_borrow_mut_lamports()? = balance;
    **account.try_borrow_mut_lamports()? = 0;
    account.resize(0)?;
    account.assign(&system_program::ID);
    Ok(())
}
