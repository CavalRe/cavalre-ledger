//! Solana host for the shared Ledger service. Owns runtime authentication,
//! account encoding/allocation, token calls and transaction rollback integration.
pub use crate::ledger_lib::{decode, root_address, LedgerError, Record, SOURCE};
use crate::ledger_lib::{to_address, MAGIC, SPACE};
use anchor_lang::{prelude::*, system_program};
use anchor_spl::token_interface::{
    self as token, Mint, TokenAccount, TokenInterface, TransferChecked,
};
use cavalre_ledger_core::{ledger as service, ledger_lib as core};
use token::spl_token_2022::extension::{
    BaseStateWithExtensions, ExtensionType, StateWithExtensions,
};

#[derive(Accounts)]
pub struct LedgerAccounts<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    /// CHECK: root identity, ownership and data authenticated by load/initialize.
    #[account(mut)]
    pub root: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    // Remaining: endpoint records, parents, custody ancestors and changed ancestors.
    // New endpoint/Source PDAs must be supplied writable for allocation.
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
    #[account(init, payer=payer, seeds=[b"vault", root.key().as_ref()], bump,
        token::mint=mint, token::authority=root, token::token_program=token_program)]
    pub vault: InterfaceAccount<'info, TokenAccount>,
    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}
#[derive(Accounts)]
pub struct MoveTokens<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,
    // Deposit payer authority. On withdrawal this signature confers no additional rights.
    pub funding_authority: Signer<'info>,
    /// CHECK: authenticated root record and signer seeds in handler.
    #[account(mut)]
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
    new: bool,
}
struct State {
    records: Vec<StoredAccount>,
}
impl State {
    fn get(&self, key: &Pubkey) -> Result<&Record> {
        self.records
            .iter()
            .find(|a| a.key == *key)
            .map(|a| &a.record)
            .ok_or(error!(LedgerError::MissingAccount))
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
            .ok_or(error!(LedgerError::MissingAccount))
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
        .ok_or(error!(LedgerError::MissingAccount))
}
fn load(root: &AccountInfo, rest: &[AccountInfo]) -> Result<State> {
    let r = decode(root)?;
    require!(
        r.depth == 2 && r.registered && r.kind == 0 && r.root == *root.key,
        LedgerError::InvalidAccount
    );
    let mut records = Vec::with_capacity(rest.len() + 1);
    records.push(StoredAccount {
        key: *root.key,
        record: r,
        changed: false,
        new: false,
    });
    for a in rest {
        require!(
            !records.iter().any(|record| record.key == *a.key),
            LedgerError::InvalidAccount
        );
        if a.owner == &crate::ID {
            let r = decode(a)?;
            require_keys_eq!(r.root, *root.key, LedgerError::InvalidAccount);
            require!(r.depth > 2, LedgerError::InvalidAccount);
            records.push(StoredAccount {
                key: *a.key,
                record: r,
                changed: false,
                new: false,
            });
        }
    }
    Ok(State { records })
}
fn save(info: &AccountInfo, r: &Record) -> Result<()> {
    require!(info.is_writable, LedgerError::InvalidAccount);
    let mut data = info.try_borrow_mut_data()?;
    data.fill(0);
    data[..8].copy_from_slice(MAGIC);
    r.serialize(&mut &mut data[8..])
        .map_err(|_| error!(LedgerError::InvalidAccount))
}
fn allocate<'info>(
    payer: &AccountInfo<'info>,
    account: &AccountInfo<'info>,
    system: &AccountInfo<'info>,
    seeds: &[&[u8]],
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
    let required = Rent::get()?
        .minimum_balance(SPACE)
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
    let signer = &[seeds];
    system_program::allocate(
        CpiContext::new_with_signer(
            *system.key,
            system_program::Allocate {
                account_to_allocate: account.clone(),
            },
            signer,
        ),
        SPACE as u64,
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
    native: Option<&'a mut MoveTokens<'info>>,
    root: service::Root<Pubkey>,
    initializing: bool,
    state: State,
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
                root_address(&scope, &identifier).0,
                *root_info.key,
                LedgerError::InvalidAccount
            );
            (
                State {
                    records: Vec::with_capacity(rest.len() + 1),
                },
                scope,
                identifier,
            )
        } else {
            let state = load(&root_info, rest)?;
            let r = state.get(root_info.key)?;
            let (scope, identifier) = (r.scope, r.identifier);
            (state, scope, identifier)
        };
        let root = service::Root {
            address: *root_info.key,
            parent: Pubkey::default(),
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
            native: None,
            root,
            state,
            initializing: initialize.is_some(),
        })
    }
    fn accounts(
        ctx: &Context<'info, LedgerAccounts<'info>>,
        initialize: Option<Pubkey>,
    ) -> Result<Self>
    where
        'info: 'a,
    {
        Self::new(
            ctx.accounts.root.to_account_info(),
            ctx.remaining_accounts,
            ctx.accounts.payer.to_account_info(),
            ctx.accounts.authority.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
            initialize.map(|id| (ctx.accounts.authority.key(), id)),
        )
    }
    fn run(mut self, command: service::Command<Pubkey>) -> Result<()> {
        service::execute(&mut self, command).map_err(|error| error.0)
    }
}
impl service::Host<Pubkey> for SolanaHost<'_, '_> {
    type Error = HostError;
    fn root(&self) -> service::Root<Pubkey> {
        self.root
    }
    fn authenticate(
        &self,
        role: service::Role,
        _command: &service::Command<Pubkey>,
    ) -> std::result::Result<Pubkey, HostError> {
        let account = match role {
            service::Role::Authority => self.authority.clone(),
            service::Role::TokenPayer => self
                .native
                .as_ref()
                .ok_or(core::Error::Unauthorized)?
                .funding_authority
                .to_account_info(),
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
        let (expected, bump) = if is_root {
            root_address(&scope, &identifier)
        } else {
            to_address(&crate::ID, &account.flags.parent, &account.relative)
        };
        if expected != key {
            return Err(core::Error::InvalidAccount.into());
        }
        let r = Record {
            root: self.root.address,
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
        };
        if let Some(existing) = self.state.records.iter_mut().find(|a| a.key == key) {
            existing.changed |= existing.record != r;
            existing.record = r;
        } else {
            self.state.records.push(StoredAccount {
                key,
                record: r,
                changed: true,
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
    fn token_balances(&mut self) -> std::result::Result<service::TokenBalances<Pubkey>, HostError> {
        let native = self.native.as_mut().ok_or(core::Error::UnsupportedToken)?;
        native.vault.reload()?;
        native.wallet.reload()?;
        Ok(service::TokenBalances {
            asset: native.mint.key(),
            owner: native.wallet.owner,
            vault: u128::from(native.vault.amount),
            wallet: u128::from(native.wallet.amount),
        })
    }
    fn move_tokens(&mut self, deposit: bool, amount: u128) -> std::result::Result<(), HostError> {
        let native = self.native.as_mut().ok_or(core::Error::UnsupportedToken)?;
        let r = self.state.get(self.root_info.key)?;
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
        Ok(())
    }
    fn commit(&mut self) -> std::result::Result<(), HostError> {
        for entry in &self.state.records {
            let (key, record) = (&entry.key, &entry.record);
            if entry.new {
                let target = info(&self.root_info, self.rest, key)?;
                let bump = [record.bump];
                let seeds: &[&[u8]] = if *key == self.root.address {
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
                allocate(&self.payer, target, &self.system, seeds)?;
            }
        }
        // Initialization previously emitted no mutation snapshots; retain that schema.
        for entry in &self.state.records {
            if entry.changed {
                let r = &entry.record;
                save(info(&self.root_info, self.rest, &entry.key)?, r)?;
                if !self.initializing {
                    emit!(AccountChanged {
                        account: entry.key,
                        root: r.root,
                        debit: r.debit,
                        credit: r.credit,
                        registered: r.registered
                    });
                }
            }
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
    ctx: &Context<'info, LedgerAccounts<'info>>,
    id: Pubkey,
    name: String,
) -> Result<()> {
    SolanaHost::accounts(ctx, Some(id))?.run(service::Command::Initialize { name })
}
pub fn add_external_token<'info>(
    ctx: Context<'info, RegisterToken<'info>>,
    name: String,
) -> Result<()> {
    validate_mint(&ctx.accounts.mint.to_account_info())?;
    validate_token_account(&ctx.accounts.vault.to_account_info())?;
    SolanaHost::new(
        ctx.accounts.root.to_account_info(),
        ctx.remaining_accounts,
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.payer.to_account_info(),
        ctx.accounts.system_program.to_account_info(),
        Some((Pubkey::default(), ctx.accounts.mint.key())),
    )?
    .run(service::Command::Initialize { name })
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
    SolanaHost::accounts(ctx, None)?.run(service::Command::Add {
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
    SolanaHost::accounts(ctx, None)?.run(service::Command::Remove {
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
    SolanaHost::accounts(ctx, None)?.run(service::Command::Transfer {
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
    host.native = Some(ctx.accounts);
    host.run(service::Command::MoveTokens {
        child: service::Child { parent, relative },
        amount: u128::from(amount),
        deposit,
    })
}

/// Snapshot after a committed mutation. This is a new Solana event schema;
/// it does not claim ERC20 Transfer-event compatibility.
#[event]
pub struct AccountChanged {
    pub account: Pubkey,
    pub root: Pubkey,
    pub debit: u128,
    pub credit: u128,
    pub registered: bool,
}

impl core::AddressDerivation<Pubkey> for SolanaHost<'_, '_> {
    fn to_address(&self, parent: &Pubkey, relative: &Pubkey) -> Pubkey {
        to_address(&crate::ID, parent, relative).0
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
        let account =
            info(&self.root_info, self.rest, key).map_err(|_| core::Error::MissingAccount)?;
        if *account.owner == system_program::ID && account.data_is_empty() {
            Ok(None)
        } else {
            Err(core::Error::InvalidAccount)
        }
    }
}
