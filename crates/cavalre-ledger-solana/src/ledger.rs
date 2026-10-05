//! Solana host for the shared Ledger service. Owns runtime authentication,
//! account encoding/allocation, token calls and transaction rollback integration.
use crate::ledger_lib::{
    child_index_address, decode_child_data, ChildSlot, CHILD_MAGIC, CHILD_SPACE,
};
pub use crate::ledger_lib::{decode, root_storage_address, LedgerError, Record, SOURCE};
use crate::ledger_lib::{global_root_address, to_address, MAGIC, NATIVE_SOL, ROOT_NAME, SPACE};
use anchor_lang::{prelude::*, system_program};
use anchor_spl::token_interface::{
    self as token, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use cavalre_ledger_core::{ledger as service, ledger_lib as core};
use std::cell::RefCell;
use token::spl_token_2022::extension::{
    BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};

#[derive(Accounts)]
pub struct LedgerAccounts<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    /// CHECK: physical root storage, not the logical ledger address. load
    /// authenticates ownership and identity. Write access only when changed.
    pub root: UncheckedAccount<'info>,
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
    /// CHECK: root identity, ownership and data authenticated by load/initialize.
    #[account(mut)]
    pub root: UncheckedAccount<'info>,
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
    pub root: UncheckedAccount<'info>,
    #[account(mint::token_program=token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(init_if_needed, payer=payer, seeds=[b"vault", root.key().as_ref()], bump,
        token::mint=mint, token::authority=root, token::token_program=token_program)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
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
    /// CHECK: authenticated root record and signer seeds in handler. Commit
    /// requires write access when balances change; zero amounts need none.
    pub root: UncheckedAccount<'info>,
    #[account(mint::token_program=token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    #[account(mut, seeds=[b"vault",root.key().as_ref()],bump,
        token::mint=mint,token::authority=root,token::token_program=token_program)]
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
    /// CHECK: canonical native root; initialized by the shared Ledger service.
    #[account(mut, seeds=[b"ledger", Pubkey::default().as_ref(), NATIVE_SOL.as_ref()], bump)]
    pub root: UncheckedAccount<'info>,
    #[account(mut, seeds=[b"vault", root.key().as_ref()], bump,
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
    /// CHECK: canonical native root; owner and record checked by the host.
    /// Commit requires write access when balances change.
    #[account(seeds=[b"ledger", Pubkey::default().as_ref(), NATIVE_SOL.as_ref()], bump)]
    pub root: UncheckedAccount<'info>,
    #[account(mut, seeds=[b"vault", root.key().as_ref()], bump,
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
fn validate_mint(mint: &AccountInfo) -> Result<()> {
    if mint.owner == &token::ID {
        let data = mint.try_borrow_data()?;
        let state = StateWithExtensions::<token::spl_token_2022::state::Mint>::unpack(&data)?;
        for extension in state.get_extension_types()? {
            require!(
                matches!(
                    extension,
                    ExtensionType::MintCloseAuthority
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
        }
    }
    Ok(())
}

fn validate_token_account(account: &AccountInfo) -> Result<()> {
    if account.owner == &token::ID {
        let data = account.try_borrow_data()?;
        let state = StateWithExtensions::<token::spl_token_2022::state::Account>::unpack(&data)?;
        for extension in state.get_extension_types()? {
            require!(
                extension == ExtensionType::ImmutableOwner,
                LedgerError::UnsupportedToken
            );
        }
    }
    Ok(())
}

struct StoredAccount {
    key: Pubkey,
    record: Record,
    changed: bool,
    metadata_changed: bool,
    new: bool,
}
struct StoredChild {
    key: Pubkey,
    slot: ChildSlot,
    changed: bool,
    new: bool,
    bump: u8,
}
struct DerivedAddress {
    parent: Pubkey,
    relative: Pubkey,
    key: Pubkey,
    bump: u8,
}
struct State {
    records: Vec<StoredAccount>,
    children: Vec<StoredChild>,
}
impl State {
    fn get(&self, key: &Pubkey) -> Result<&Record> {
        self.records
            .iter()
            .find(|a| a.key == *key)
            .map(|a| &a.record)
            .ok_or_else(|| error!(LedgerError::MissingAccount))
    }
    fn optional(&self, key: &Pubkey) -> Option<&Record> {
        self.records
            .iter()
            .find(|a| a.key == *key)
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
fn load(root: &AccountInfo, rest: &[AccountInfo]) -> Result<State> {
    let r = decode(root)?;
    require!(
        r.depth == 2 && r.registered && r.kind == 0,
        LedgerError::InvalidAccount
    );
    let mut records = Vec::with_capacity(rest.len() + 1);
    let mut children = Vec::new();
    let ledger = r.root;
    records.push(StoredAccount {
        key: ledger,
        record: r,
        changed: false,
        metadata_changed: false,
        new: false,
    });
    for a in rest {
        if a.owner == &crate::ID {
            require!(
                a.key != root.key
                    && !records.iter().any(|record| record.key == *a.key)
                    && !children.iter().any(|slot: &StoredChild| slot.key == *a.key),
                LedgerError::InvalidAccount
            );
            if a.try_borrow_data()?.get(..8) == Some(CHILD_MAGIC) {
                children.push(StoredChild {
                    key: *a.key,
                    slot: decode_child_data(a.key, a.owner, &a.try_borrow_data()?)?,
                    changed: false,
                    new: false,
                    bump: 0, // Existing slots need no allocation signer.
                });
                continue;
            }
            let r = decode(a)?;
            require_keys_eq!(r.root, ledger, LedgerError::InvalidAccount);
            require!(r.depth > 2, LedgerError::InvalidAccount);
            records.push(StoredAccount {
                key: *a.key,
                record: r,
                changed: false,
                metadata_changed: false,
                new: false,
            });
        }
    }
    Ok(State { records, children })
}
// Borsh offsets within the unchanged 512-byte record. The variable-length name
// precedes sub_index; balance and child-count fields are in the fixed prefix.
const CHILDREN_OFFSET: usize = 8 + 4 * 32 + 5;
const BALANCES_OFFSET: usize = CHILDREN_OFFSET + 4;
const SUB_INDEX_OFFSET: usize = BALANCES_OFFSET + 2 * 16 + 4 + 2 * 32 + 1;
// Root-only immutable custody identity, outside Record's Borsh payload (at
// most 383 bytes including MAGIC). Uses existing allocation, never leaf state.
const VAULT_OFFSET: usize = SPACE - 32;

fn save_fields(data: &mut [u8], r: &Record) {
    data[CHILDREN_OFFSET..CHILDREN_OFFSET + 4].copy_from_slice(&r.children.to_le_bytes());
    data[BALANCES_OFFSET..BALANCES_OFFSET + 16].copy_from_slice(&r.debit.to_le_bytes());
    data[BALANCES_OFFSET + 16..BALANCES_OFFSET + 32].copy_from_slice(&r.credit.to_le_bytes());
    let index = SUB_INDEX_OFFSET + r.name.len();
    data[index..index + 4].copy_from_slice(&r.sub_index.to_le_bytes());
}

fn save(info: &AccountInfo, r: &Record, metadata_changed: bool) -> Result<()> {
    require!(info.is_writable, LedgerError::InvalidAccount);
    let mut data = info.try_borrow_mut_data()?;
    if !metadata_changed {
        save_fields(&mut data, r);
        return Ok(());
    }
    // Preserve the root's immutable custody identity when metadata is saved.
    data[..VAULT_OFFSET].fill(0);
    data[..8].copy_from_slice(MAGIC);
    r.serialize(&mut &mut data[8..VAULT_OFFSET])
        .map_err(|_| error!(LedgerError::InvalidAccount))
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
    Tokens(&'a mut MoveTokens<'info>),
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
                root_storage_address(&scope, &identifier).0,
                *root_info.key,
                LedgerError::InvalidAccount
            );
            (
                if root_info.data_is_empty() {
                    State {
                        records: Vec::with_capacity(rest.len() + 2),
                        children: Vec::new(),
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
            let (scope, identifier) = (r.scope, r.identifier);
            (state, scope, identifier)
        };
        let address = if scope == Pubkey::default() {
            identifier
        } else {
            *root_info.key
        };
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
        // Loaded records and this instruction's newly derived addresses are
        // authenticated. Never cache an address merely supplied by the caller.
        if let Some(entry) = self.state.records.iter().find(|entry| {
            entry.record.depth > 2
                && entry.record.parent == *parent
                && entry.record.relative == *relative
        }) {
            return (entry.key, entry.record.bump);
        }
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
            ctx.accounts.root.to_account_info(),
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
                    StateWithExtensions::<token::spl_token_2022::state::Account>::unpack(&data)?;
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
                        extension == ExtensionType::ImmutableOwner,
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
        // new() authenticated the root's owner, PDA and full record before this
        // read. Only verified registration writes this immutable address.
        let data = self.root_info.try_borrow_data()?;
        let vault = Pubkey::new_from_array(
            data[VAULT_OFFSET..SPACE]
                .try_into()
                .map_err(|_| error!(LedgerError::InvalidAccount))?,
        );
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
            let record = decode(&global)?;
            require!(record.depth == 1, LedgerError::InvalidAccount);
            self.state.records.push(StoredAccount {
                key: *global.key,
                record,
                changed: false,
                metadata_changed: false,
                new: false,
            });
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
        self.state
            .children
            .iter()
            .find(|a| a.slot.parent == *parent && a.slot.index == index)
            .ok_or(core::Error::MissingAccount)?
            .slot
            .relative
            .ok_or(core::Error::InvalidIndex)
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
                Some(Settlement::Tokens(accounts)) => accounts.funding_authority.to_account_info(),
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
        let is_global = key == self.root.parent;
        let (expected, bump) = if is_global {
            global_root_address()
        } else if is_root {
            root_storage_address(&scope, &identifier)
        } else {
            self.child_address(&account.flags.parent, &account.relative)
        };
        let logical = if is_root && scope == Pubkey::default() {
            identifier
        } else {
            expected
        };
        if logical != key {
            return Err(core::Error::InvalidAccount.into());
        }
        let r = Record {
            root: if is_global { key } else { self.root.address },
            parent: account.flags.parent,
            relative: account.relative,
            custodian: account.custodian,
            kind: match account.flags.account_kind {
                core::AccountKind::DebitGroup => 0,
                core::AccountKind::CreditGroup => 1,
                core::AccountKind::DebitLedger => 2,
                core::AccountKind::CreditLedger => 3,
            },
            token_kind: if is_root {
                if self.root.authority.is_some() {
                    3
                } else if identifier == NATIVE_SOL {
                    1
                } else {
                    2
                }
            } else {
                0
            },
            depth: account.flags.depth,
            registered: account.registered,
            implicit_allowed: account.implicit_allowed,
            children: account.children,
            debit: account.balances.debit,
            credit: account.balances.credit,
            name: account.name,
            scope,
            identifier,
            bump,
            sub_index: account.sub_index,
            symbol: account.symbol,
            decimals: account.decimals,
        };
        if let Some(existing) = self.state.records.iter_mut().find(|a| a.key == key) {
            let changed = existing.record != r;
            existing.changed |= changed;
            existing.metadata_changed |= changed;
            existing.record = r;
        } else {
            self.state.records.push(StoredAccount {
                key,
                record: r,
                changed: true,
                metadata_changed: true,
                new: true,
            });
        }
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
        Ok(())
    }
    fn set_children(&mut self, key: Pubkey, children: u32) -> std::result::Result<(), HostError> {
        let entry = self.state.entry_mut(&key)?;
        entry.changed |= entry.record.children != children;
        entry.record.children = children;
        Ok(())
    }
    fn set_sub_index(&mut self, key: Pubkey, index: u32) -> std::result::Result<(), HostError> {
        let entry = self.state.entry_mut(&key)?;
        entry.changed |= entry.record.sub_index != index;
        entry.record.sub_index = index;
        Ok(())
    }
    fn set_child(
        &mut self,
        parent: Pubkey,
        index: u32,
        relative: Option<Pubkey>,
    ) -> std::result::Result<(), HostError> {
        if let Some(entry) = self
            .state
            .children
            .iter_mut()
            .find(|a| a.slot.parent == parent && a.slot.index == index)
        {
            entry.changed |= entry.slot.relative != relative;
            entry.slot.relative = relative;
        } else {
            let (key, bump) = child_index_address(&parent, index);
            let target = self.account_info(&key)?;
            let new = target.data_is_empty();
            if !new {
                decode_child_data(
                    &key,
                    target.owner,
                    &target
                        .try_borrow_data()
                        .map_err(anchor_lang::error::Error::from)?,
                )?;
            }
            self.state.children.push(StoredChild {
                key,
                slot: ChildSlot {
                    parent,
                    index,
                    relative,
                },
                changed: true,
                new,
                bump,
            });
        }
        Ok(())
    }
    fn token_balances(&mut self) -> std::result::Result<service::TokenBalances<Pubkey>, HostError> {
        match self
            .settlement
            .as_mut()
            .ok_or(core::Error::UnsupportedToken)?
        {
            Settlement::Tokens(native) => Ok(service::TokenBalances {
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
        let native = match settlement {
            Settlement::Tokens(accounts) => accounts,
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
        let r = self.state.get(&self.root.address)?;
        let bump = [r.bump];
        let seeds: &[&[u8]] = &[b"ledger", r.scope.as_ref(), r.identifier.as_ref(), &bump];
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
        // Anchor decoded the initial balances. Refresh only after the CPI so
        // the core still observes actual token-program settlement on both sides.
        native.vault.reload()?;
        native.wallet.reload()?;
        Ok(())
    }
    fn commit(&mut self) -> std::result::Result<(), HostError> {
        for entry in &self.state.records {
            let (key, record) = (&entry.key, &entry.record);
            if entry.new {
                let target = self.account_info(key)?;
                let bump = [record.bump];
                let seeds: &[&[u8]] = if record.depth == 1 {
                    &[ROOT_NAME.as_bytes(), &bump]
                } else if *key == self.root.address {
                    &[
                        b"ledger",
                        record.scope.as_ref(),
                        record.identifier.as_ref(),
                        &bump,
                    ]
                } else {
                    &[
                        b"account",
                        record.parent.as_ref(),
                        record.relative.as_ref(),
                        &bump,
                    ]
                };
                allocate(&self.payer, target, &self.system, seeds, SPACE)?;
            }
        }
        for entry in &self.state.children {
            if !entry.changed {
                continue;
            }
            let target = self.account_info(&entry.key)?;
            if entry.new {
                let bump = [entry.bump];
                let index = entry.slot.index.to_le_bytes();
                let seeds: &[&[u8]] = &[b"subs", entry.slot.parent.as_ref(), &index, &bump];
                allocate(&self.payer, target, &self.system, seeds, CHILD_SPACE)?;
            }
            if !target.is_writable {
                return Err(core::Error::InvalidAccount.into());
            }
            let mut data = target
                .try_borrow_mut_data()
                .map_err(anchor_lang::error::Error::from)?;
            data.fill(0);
            data[..8].copy_from_slice(CHILD_MAGIC);
            entry
                .slot
                .serialize(&mut &mut data[8..])
                .map_err(|_| error!(LedgerError::InvalidAccount))?;
        }
        for entry in &self.state.records {
            if entry.changed {
                let r = &entry.record;
                save(self.account_info(&entry.key)?, r, entry.metadata_changed)?;
            }
        }
        if let Some(vault) = self.initialize_vault {
            if !self.root_info.is_writable {
                return Err(core::Error::InvalidAccount.into());
            }
            self.root_info
                .try_borrow_mut_data()
                .map_err(anchor_lang::error::Error::from)?[VAULT_OFFSET..SPACE]
                .copy_from_slice(vault.as_ref());
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
        ctx.accounts.root.to_account_info(),
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
    validate_token_account(&ctx.accounts.vault.to_account_info())?;
    let (name, symbol, decimals) = crate::ledger_view::registration_metadata(
        &ctx.accounts.mint.to_account_info(),
        ctx.remaining_accounts,
    )?;
    let mut host = SolanaHost::new(
        ctx.accounts.root.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        Some((Pubkey::default(), ctx.accounts.mint.key())),
    )?
    .with_backing(
        ctx.accounts.mint.key(),
        u128::from(ctx.accounts.vault.amount),
    )
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
    if !ctx.accounts.root.data_is_empty() {
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
        ctx.accounts.root.to_account_info(),
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
    implicit_allowed: bool,
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
        implicit_allowed,
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
pub fn transfer<'info>(
    ctx: &Context<'info, LedgerAccounts<'info>>,
    from_parent: Pubkey,
    from: Pubkey,
    to_parent: Pubkey,
    to: Pubkey,
    amount: u128,
) -> Result<()> {
    SolanaHost::accounts(ctx)?.run(service::Command::Transfer {
        from: service::Child {
            parent: from_parent,
            relative: from,
        },
        to: service::Child {
            parent: to_parent,
            relative: to,
        },
        amount,
    })
}
pub fn move_tokens<'info>(
    ctx: Context<'info, MoveTokens<'info>>,
    parent: Pubkey,
    relative: Pubkey,
    amount: u64,
    deposit: bool,
) -> Result<()> {
    validate_mint(&ctx.accounts.mint.to_account_info())?;
    validate_token_account(&ctx.accounts.vault.to_account_info())?;
    validate_token_account(&ctx.accounts.wallet.to_account_info())?;
    let mut host = SolanaHost::new(
        ctx.accounts.root.to_account_info(),
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
    host.settlement = Some(Settlement::Tokens(ctx.accounts));
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
        ctx.accounts.root.to_account_info(),
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
        if let Some(record) = self.state.optional(key) {
            return Ok(Some(record.borrowed()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_writes_match_full_borsh_encoding_for_variable_metadata() {
        fn encoded(record: &Record) -> Vec<u8> {
            let mut data = vec![0; SPACE];
            data[..8].copy_from_slice(MAGIC);
            record.serialize(&mut &mut data[8..]).unwrap();
            data
        }
        for name in [String::new(), "é".repeat(17), "N".repeat(64)] {
            for symbol in ["", "TOK", &"S".repeat(64)] {
                let mut record = Record {
                    root: Pubkey::new_unique(),
                    parent: Pubkey::new_unique(),
                    relative: Pubkey::new_unique(),
                    custodian: Pubkey::new_unique(),
                    kind: 2,
                    token_kind: 0,
                    depth: 4,
                    registered: true,
                    implicit_allowed: true,
                    children: 3,
                    debit: 17,
                    credit: 29,
                    name: name.clone(),
                    scope: Pubkey::new_unique(),
                    identifier: Pubkey::new_unique(),
                    bump: 251,
                    sub_index: 1,
                    symbol: symbol.into(),
                    decimals: 9,
                };
                let mut data = encoded(&record);
                for (children, debit, credit, index) in [
                    (u32::MAX, u128::MAX, u128::MAX, u32::MAX),
                    (0, 0, 0, 0),
                    (7, 1 << 100, 1 << 90, 2),
                ] {
                    record.children = children;
                    record.debit = debit;
                    record.credit = credit;
                    record.sub_index = index;
                    save_fields(&mut data, &record);
                    assert_eq!(data, encoded(&record));
                }
            }
        }
    }
}
