//! A second host with integer identities and transactional memory storage.
//! No Solana SDK, account layout, signing key or token program is involved.
use cavalre_ledger_core::{
    ledger::{execute, Account, Child, Command, Event, Host, Role, Root, TokenBalances},
    ledger_lib::{AccountKind, Balances, Error},
};
use std::{cell::Cell, collections::BTreeMap};

const APP: u64 = 10;
const PAYER: u64 = 70;
const USER: u64 = 20;
const ROOT: u64 = 1;
const SOURCE: u64 = 83;
fn address(parent: u64, relative: u64) -> u64 {
    parent * 256 + relative
}
fn child(parent: u64, relative: u64) -> Child<u64> {
    Child { parent, relative }
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Rule(Error),
    Commit,
    Native,
}
impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Rule(error)
    }
}

struct MemoryHost {
    root: Root<u64>,
    accounts: BTreeMap<u64, Account<u64>>,
    events: Vec<Event<u64>>,
    tokens: TokenBalances<u64>,
    // Trusted execution context. These are verified runtime identities, not
    // command arguments or a suggested implementation of cryptography.
    actor: Option<u64>,
    payer: Option<u64>,
    active: bool,
    payer_checks: Cell<usize>,
    fail_commit: bool,
    short_receipt: bool,
    authenticated_deposit: Option<u128>,
}
impl MemoryHost {
    fn new(internal: bool) -> Self {
        Self {
            root: Root {
                address: ROOT,
                parent: 0,
                identifier: 99,
                source: SOURCE,
                authority: internal.then_some(APP),
            },
            accounts: BTreeMap::new(),
            events: Vec::new(),
            tokens: TokenBalances {
                asset: 99,
                owner: PAYER,
                vault: 0,
                wallet: 1000,
            },
            actor: Some(APP),
            payer: Some(PAYER),
            active: false,
            payer_checks: Cell::new(0),
            fail_commit: false,
            short_receipt: false,
            authenticated_deposit: None,
        }
    }
    fn initialize(&mut self, implicit: bool) -> u64 {
        execute(
            self,
            Command::Initialize {
                name: "Ledger".into(),
            },
        )
        .unwrap();
        execute(
            self,
            Command::Add {
                child: child(ROOT, APP),
                name: "App".into(),
                kind: AccountKind::DebitGroup,
                implicit_allowed: implicit,
            },
        )
        .unwrap();
        address(ROOT, APP)
    }
    fn move_funds(&mut self, parent: u64, amount: u128, deposit: bool) -> Result<(), Failure> {
        execute(
            self,
            Command::MoveTokens {
                child: child(parent, USER),
                amount,
                deposit,
            },
        )
    }
    fn balance(&self, absolute: u64) -> Balances {
        self.accounts[&absolute].balances
    }
}
impl Host<u64> for MemoryHost {
    type Error = Failure;
    fn root(&self) -> Root<u64> {
        self.root
    }
    fn authenticate(&self, role: Role, command: &Command<u64>) -> Result<u64, Failure> {
        assert!(self.active, "authentication must be inside the transaction");
        let identity = match role {
            Role::Authority => self.actor,
            Role::TokenPayer => {
                if let Some(expected) = self.authenticated_deposit {
                    if !matches!(command, Command::MoveTokens { amount, deposit: true, .. } if *amount == expected)
                    {
                        return Err(Error::Unauthorized.into());
                    }
                }
                self.payer_checks.set(self.payer_checks.get() + 1);
                self.payer
            }
        };
        identity.ok_or(Failure::Rule(Error::Unauthorized))
    }
    fn put(&mut self, absolute: u64, account: Account<u64>) -> Result<(), Failure> {
        assert!(self.active);
        self.accounts.insert(absolute, account);
        Ok(())
    }
    fn token_balances(&mut self) -> Result<TokenBalances<u64>, Failure> {
        Ok(self.tokens)
    }
    fn set_balances(&mut self, address: u64, balances: Balances) -> Result<(), Failure> {
        assert!(self.active);
        self.accounts
            .get_mut(&address)
            .ok_or(Error::MissingAccount)?
            .balances = balances;
        Ok(())
    }
    fn set_children(&mut self, address: u64, children: u32) -> Result<(), Failure> {
        assert!(self.active);
        self.accounts
            .get_mut(&address)
            .ok_or(Error::MissingAccount)?
            .children = children;
        Ok(())
    }
    fn move_tokens(&mut self, deposit: bool, amount: u128) -> Result<(), Failure> {
        assert!(self.active);
        if deposit {
            self.tokens.wallet = self
                .tokens
                .wallet
                .checked_sub(amount)
                .ok_or(Failure::Native)?;
            self.tokens.vault += amount - u128::from(self.short_receipt && amount > 0);
        } else {
            self.tokens.vault = self
                .tokens
                .vault
                .checked_sub(amount)
                .ok_or(Failure::Native)?;
            self.tokens.wallet += amount;
        }
        Ok(())
    }
    fn commit(&mut self) -> Result<(), Failure> {
        assert!(self.active);
        if self.fail_commit {
            Err(Failure::Commit)
        } else {
            Ok(())
        }
    }
    fn emit(&mut self, event: Event<u64>) -> Result<(), Failure> {
        assert!(self.active);
        self.events.push(event);
        Ok(())
    }
    fn atomic(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        assert!(!self.active);
        let accounts = self.accounts.clone();
        let tokens = self.tokens;
        let events = self.events.len();
        self.active = true;
        let result = operation(self);
        if result.is_err() {
            self.accounts = accounts;
            self.tokens = tokens;
            self.events.truncate(events);
        }
        self.active = false;
        result
    }
}

#[test]
fn every_command_requires_a_host_authenticated_actor() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    host.actor = None;
    let before = host.accounts.clone();
    for command in [
        Command::Initialize {
            name: "Ledger".into(),
        },
        Command::Add {
            child: child(ROOT, APP),
            name: "App".into(),
            kind: AccountKind::DebitGroup,
            implicit_allowed: true,
        },
        Command::Remove {
            child: child(parent, USER),
            group: false,
        },
        Command::Transfer {
            from: child(parent, USER),
            to: child(parent, USER),
            amount: 0,
        },
        Command::MoveTokens {
            child: child(parent, USER),
            amount: 0,
            deposit: true,
        },
        Command::MoveTokens {
            child: child(parent, USER),
            amount: 0,
            deposit: false,
        },
    ] {
        assert_eq!(
            execute(&mut host, command),
            Err(Failure::Rule(Error::Unauthorized))
        );
        assert_eq!(host.accounts, before);
    }
}

#[test]
fn core_uses_application_identity_and_distinct_payer_consent() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    host.payer = Some(APP); // Application authentication cannot stand in for payer consent.
    assert_eq!(
        host.move_funds(parent, 40, true),
        Err(Failure::Rule(Error::Unauthorized))
    );
    host.payer = None;
    assert_eq!(
        host.move_funds(parent, 40, true),
        Err(Failure::Rule(Error::Unauthorized))
    );
    host.payer = Some(PAYER);
    host.actor = Some(PAYER); // Conversely, token ownership cannot authorize the app branch.
    assert_eq!(
        host.move_funds(parent, 40, true),
        Err(Failure::Rule(Error::Unauthorized))
    );
    host.actor = Some(APP);
    host.move_funds(parent, 40, true).unwrap();
    assert_eq!(host.balance(address(parent, USER)).debit, 40);
    assert!(!host.accounts[&address(parent, USER)].registered);
    assert_eq!(
        host.balance(ROOT),
        Balances {
            debit: 40,
            credit: 40
        }
    );
    assert_eq!(host.balance(address(ROOT, SOURCE)).credit, 40);
    let checks = host.payer_checks.get();
    host.payer = None;
    host.move_funds(parent, 40, false).unwrap();
    assert_eq!(
        host.payer_checks.get(),
        checks,
        "withdrawal must not demand the recipient's signature"
    );
    assert_eq!(host.balance(ROOT), Balances::default());
    assert_eq!((host.tokens.wallet, host.tokens.vault), (1000, 0));
}

#[test]
fn core_owns_parent_admission_registration_and_removal() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(false);
    assert_eq!(
        host.move_funds(parent, 10, true),
        Err(Failure::Rule(Error::InvalidAccount))
    );
    execute(
        &mut host,
        Command::Add {
            child: child(parent, USER),
            name: "User".into(),
            kind: AccountKind::DebitLedger,
            implicit_allowed: true,
        },
    )
    .unwrap();
    assert_eq!(host.accounts[&parent].children, 1);
    host.move_funds(parent, 10, true).unwrap();
    assert_eq!(
        execute(
            &mut host,
            Command::Remove {
                child: child(parent, USER),
                group: false
            }
        ),
        Err(Failure::Rule(Error::NonemptyAccount))
    );
    assert_eq!(
        execute(
            &mut host,
            Command::Add {
                child: child(parent, USER),
                name: "Changed".into(),
                kind: AccountKind::DebitLedger,
                implicit_allowed: true,
            }
        ),
        Err(Failure::Rule(Error::MetadataConflict))
    );
    host.move_funds(parent, 10, false).unwrap();
    execute(
        &mut host,
        Command::Remove {
            child: child(parent, USER),
            group: false,
        },
    )
    .unwrap();
    assert!(!host.accounts[&address(parent, USER)].registered);
    assert_eq!(host.accounts[&parent].children, 0);
    // The same absent account may be removed, but it cannot receive under this policy.
    execute(
        &mut host,
        Command::Remove {
            child: child(parent, USER),
            group: false,
        },
    )
    .unwrap();
    assert_eq!(
        host.move_funds(parent, 10, true),
        Err(Failure::Rule(Error::InvalidAccount))
    );
}

#[test]
fn host_transaction_rolls_back_native_movement_and_all_ledger_writes() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    let before = host.accounts.clone();
    let events = host.events.clone();
    host.fail_commit = true;
    assert_eq!(host.move_funds(parent, 40, true), Err(Failure::Commit));
    assert_eq!(host.accounts, before);
    assert_eq!(host.events, events);
    assert_eq!((host.tokens.wallet, host.tokens.vault), (1000, 0));
    host.fail_commit = false;
    host.move_funds(parent, 40, true).unwrap();
    let funded = host.accounts.clone();
    let events = host.events.clone();
    host.fail_commit = true;
    assert_eq!(host.move_funds(parent, 40, false), Err(Failure::Commit));
    assert_eq!(host.accounts, funded);
    assert_eq!(host.events, events);
    assert_eq!((host.tokens.wallet, host.tokens.vault), (960, 40));
}

#[test]
fn core_requires_exact_settlement_and_full_liability_backing() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    let before = host.accounts.clone();
    host.short_receipt = true;
    assert_eq!(
        host.move_funds(parent, 40, true),
        Err(Failure::Rule(Error::Settlement))
    );
    assert_eq!(host.accounts, before);
    assert_eq!((host.tokens.wallet, host.tokens.vault), (1000, 0));
    host.short_receipt = false;
    host.move_funds(parent, 40, true).unwrap();
    host.tokens.vault = 20;
    let before = host.accounts.clone();
    assert_eq!(
        host.move_funds(parent, 1, false),
        Err(Failure::Rule(Error::Undercollateralized))
    );
    assert_eq!(host.accounts, before);
    assert_eq!(host.tokens.vault, 20);
}

#[test]
fn core_scopes_tree_permissions_and_protects_source_and_external_polarity() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    host.actor = Some(11);
    assert_eq!(
        execute(
            &mut host,
            Command::Add {
                child: child(ROOT, APP),
                name: "App".into(),
                kind: AccountKind::DebitGroup,
                implicit_allowed: true,
            }
        ),
        Err(Failure::Rule(Error::Unauthorized))
    );
    assert_eq!(
        execute(
            &mut host,
            Command::Remove {
                child: child(parent, USER),
                group: false
            }
        ),
        Err(Failure::Rule(Error::Unauthorized))
    );
    host.actor = Some(APP);
    assert_eq!(
        execute(
            &mut host,
            Command::Add {
                child: child(parent, USER),
                name: "Credit".into(),
                kind: AccountKind::CreditLedger,
                implicit_allowed: true,
            }
        ),
        Err(Failure::Rule(Error::InvalidKind))
    );
    assert_eq!(
        execute(
            &mut host,
            Command::Remove {
                child: child(ROOT, SOURCE),
                group: false
            }
        ),
        Err(Failure::Rule(Error::Unauthorized))
    );
}

#[test]
fn internal_issuance_uses_root_authority_and_never_native_custody() {
    let mut host = MemoryHost::new(true);
    let parent = host.initialize(true);
    host.actor = Some(USER);
    assert_eq!(
        execute(
            &mut host,
            Command::Transfer {
                from: child(ROOT, SOURCE),
                to: child(parent, USER),
                amount: u128::MAX,
            }
        ),
        Err(Failure::Rule(Error::Unauthorized))
    );
    host.actor = Some(APP);
    execute(
        &mut host,
        Command::Transfer {
            from: child(ROOT, SOURCE),
            to: child(parent, USER),
            amount: u128::MAX,
        },
    )
    .unwrap();
    assert_eq!(
        host.balance(ROOT),
        Balances {
            debit: u128::MAX,
            credit: u128::MAX
        }
    );
    assert_eq!((host.tokens.wallet, host.tokens.vault), (1000, 0));
    assert_eq!(
        host.move_funds(parent, 1, false),
        Err(Failure::Rule(Error::UnsupportedToken))
    );
    execute(
        &mut host,
        Command::Transfer {
            from: child(parent, USER),
            to: child(ROOT, SOURCE),
            amount: u128::MAX,
        },
    )
    .unwrap();
    assert_eq!(host.balance(ROOT), Balances::default());
}

#[test]
fn host_authentication_receives_the_requested_funding_terms() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    // Model a host whose current authenticated funding context authorizes 40.
    // This is a host contract test, not an intent protocol or signature algorithm.
    host.authenticated_deposit = Some(40);
    assert_eq!(
        host.move_funds(parent, 41, true),
        Err(Failure::Rule(Error::Unauthorized))
    );
    assert_eq!((host.tokens.wallet, host.tokens.vault), (1000, 0));
    host.move_funds(parent, 40, true).unwrap();
    assert_eq!(host.balance(address(parent, USER)).debit, 40);
}

impl cavalre_ledger_core::ledger_lib::AddressDerivation<u64> for MemoryHost {
    fn to_address(&self, parent: &u64, relative: &u64) -> u64 {
        address(*parent, *relative)
    }
}

impl cavalre_ledger_core::ledger_lib::ReadStore<u64> for MemoryHost {
    fn account(&self, absolute: &u64) -> Result<Option<Account<u64, &str>>, Error> {
        Ok(self.accounts.get(absolute).map(Account::as_ref))
    }
}

#[test]
fn lifecycle_events_preserve_source_order_relative_identity_and_silent_noops() {
    let mut host = MemoryHost::new(true);
    let parent = host.initialize(true);
    assert_eq!(
        host.events,
        vec![
            Event::SubAccountAdded {
                ledger: ROOT,
                parent: ROOT,
                relative: SOURCE,
                is_credit: true
            },
            Event::LedgerAdded {
                ledger: ROOT,
                authority: Some(APP),
                identifier: 99,
                name: "Ledger".into()
            },
            Event::SubAccountGroupAdded {
                ledger: ROOT,
                parent: ROOT,
                relative: APP,
                name: "App".into(),
                is_credit: false
            },
        ]
    );
    host.events.clear();
    let add = || Command::Add {
        child: child(parent, USER),
        name: "User".into(),
        kind: AccountKind::DebitLedger,
        implicit_allowed: true,
    };
    execute(&mut host, add()).unwrap();
    assert_eq!(
        host.events,
        vec![Event::SubAccountAdded {
            ledger: ROOT,
            parent,
            relative: USER,
            is_credit: false
        }]
    );
    host.events.clear();
    execute(&mut host, add()).unwrap();
    assert!(host.events.is_empty());
    host.actor = Some(USER);
    assert!(execute(&mut host, add()).is_err());
    assert!(host.events.is_empty());
    host.actor = Some(APP);
    execute(
        &mut host,
        Command::Remove {
            child: child(parent, USER),
            group: false,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![Event::SubAccountRemoved {
            ledger: ROOT,
            parent,
            relative: USER
        }]
    );
    host.events.clear();
    execute(
        &mut host,
        Command::Remove {
            child: child(parent, USER),
            group: false,
        },
    )
    .unwrap();
    assert!(host.events.is_empty());
    execute(
        &mut host,
        Command::Remove {
            child: child(ROOT, APP),
            group: true,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![Event::SubAccountGroupRemoved {
            ledger: ROOT,
            parent: ROOT,
            relative: APP
        }]
    );
    host.events.clear();
    execute(
        &mut host,
        Command::Remove {
            child: child(ROOT, APP),
            group: true,
        },
    )
    .unwrap();
    assert!(host.events.is_empty());
}

fn credit(account: u64, amount: u128, balance: u128) -> Event<u64> {
    Event::Credit {
        ledger: ROOT,
        account,
        amount,
        balance,
    }
}
fn debit(account: u64, amount: u128, balance: u128) -> Event<u64> {
    Event::Debit {
        ledger: ROOT,
        account,
        amount,
        balance,
    }
}

#[test]
fn funding_events_preserve_unequal_depth_order_columns_and_implicit_identity() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    let source = address(ROOT, SOURCE);
    let user = address(parent, USER);
    host.events.clear();
    host.move_funds(parent, 40, true).unwrap();
    assert_eq!(
        host.events,
        vec![
            debit(user, 40, 40),
            credit(source, 40, 40),
            debit(parent, 40, 40),
            credit(ROOT, 40, 40),
            debit(ROOT, 40, 40),
        ]
    );
    assert!(!host.accounts[&user].registered);
    host.events.clear();
    host.move_funds(parent, 10, false).unwrap();
    assert_eq!(
        host.events,
        vec![
            credit(user, 10, 30),
            credit(parent, 10, 30),
            debit(source, 10, 30),
            credit(ROOT, 10, 30),
            debit(ROOT, 10, 30),
        ]
    );
}

#[test]
fn posting_events_cancel_shared_ancestors_keep_zero_postings_and_skip_self_transfers() {
    let mut host = MemoryHost::new(false);
    let parent = host.initialize(true);
    host.move_funds(parent, 40, true).unwrap();
    let from = child(parent, USER);
    let to = child(parent, USER + 1);
    host.events.clear();
    execute(
        &mut host,
        Command::Transfer {
            from,
            to,
            amount: 10,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![
            credit(address(parent, USER), 10, 30),
            debit(address(parent, USER + 1), 10, 10)
        ]
    );
    host.events.clear();
    execute(
        &mut host,
        Command::Transfer {
            from,
            to,
            amount: 0,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![
            credit(address(parent, USER), 0, 30),
            debit(address(parent, USER + 1), 0, 10)
        ]
    );
    host.events.clear();
    execute(
        &mut host,
        Command::Transfer {
            from,
            to: from,
            amount: 30,
        },
    )
    .unwrap();
    assert!(host.events.is_empty());
    assert!(execute(
        &mut host,
        Command::Transfer {
            from,
            to: from,
            amount: 31
        }
    )
    .is_err());
    assert!(host.events.is_empty());
}

#[test]
fn opposite_polarity_events_follow_leaf_columns_through_the_same_group() {
    let mut host = MemoryHost::new(true);
    let parent = host.initialize(true); // Debit group containing both account kinds.
    let from = child(parent, USER);
    let to = child(parent, USER + 1);
    execute(
        &mut host,
        Command::Add {
            child: from,
            name: "Credit".into(),
            kind: AccountKind::CreditLedger,
            implicit_allowed: true,
        },
    )
    .unwrap();
    host.events.clear();
    execute(
        &mut host,
        Command::Transfer {
            from,
            to,
            amount: u128::MAX,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![
            credit(address(parent, USER), u128::MAX, u128::MAX),
            debit(address(parent, USER + 1), u128::MAX, u128::MAX),
            credit(parent, u128::MAX, u128::MAX),
            debit(parent, u128::MAX, u128::MAX),
            credit(ROOT, u128::MAX, u128::MAX),
            debit(ROOT, u128::MAX, u128::MAX),
        ]
    );
    host.events.clear();
    assert!(execute(
        &mut host,
        Command::Transfer {
            from,
            to,
            amount: 1
        }
    )
    .is_err());
    assert!(host.events.is_empty());
    execute(
        &mut host,
        Command::Transfer {
            from: to,
            to: from,
            amount: u128::MAX,
        },
    )
    .unwrap();
    assert_eq!(
        host.events,
        vec![
            credit(address(parent, USER + 1), u128::MAX, 0),
            debit(address(parent, USER), u128::MAX, 0),
            credit(parent, u128::MAX, 0),
            debit(parent, u128::MAX, 0),
            credit(ROOT, u128::MAX, 0),
            debit(ROOT, u128::MAX, 0),
        ]
    );
}
