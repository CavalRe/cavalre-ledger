use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use cavalre_ledger_solana::{self as ledger, accounts, instruction};
use ledger::ledger::{Record, SOURCE};
use litesvm::LiteSVM;
use solana_account::Account;
use solana_address::{address, Address};
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_program_option::COption;
use solana_program_pack::Pack;
use solana_signer::Signer;
use solana_transaction::Transaction;
use spl_token_interface::state::{Account as TokenAccount, AccountState, Mint};
mod acceptance;
const SYSTEM: Address = address!("11111111111111111111111111111111");
const TOKEN: Address = address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022: Address = address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
fn ap(a: Address) -> anchor_lang::prelude::Pubkey {
    anchor_lang::prelude::Pubkey::new_from_array(a.to_bytes())
}
fn sa(a: anchor_lang::prelude::Pubkey) -> Address {
    Address::new_from_array(a.to_bytes())
}
fn child(parent: Address, relative: Address) -> Address {
    sa(ledger::ledger_lib::to_address(&ap(parent), &ap(relative)))
}
fn ix(a: impl ToAccountMetas, d: impl InstructionData, rest: &[Address]) -> Instruction {
    use anchor_lang::Discriminator;
    let data = d.data();
    let mut accounts: Vec<_> = a
        .to_account_metas(None)
        .into_iter()
        .map(|m| AccountMeta {
            pubkey: sa(m.pubkey),
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        })
        .collect();
    // Existing acceptance fixtures retain their conservative write declarations.
    // writable_accounts tests construct minimal permissions with Reader's planner.
    if accounts.len() == 4 {
        accounts[2].is_writable = true;
    }
    // Existing custody fixtures also retain their root write declaration.
    // Zero-amount tests explicitly downgrade every Ledger record to read-only.
    if [
        instruction::Wrap::DISCRIMINATOR,
        instruction::Unwrap::DISCRIMINATOR,
        instruction::WrapSol::DISCRIMINATOR,
        instruction::UnwrapSol::DISCRIMINATOR,
    ]
    .iter()
    .any(|discriminator| data.starts_with(discriminator))
    {
        accounts[3].is_writable = true;
    }
    accounts.extend(rest.iter().map(|k| AccountMeta::new(*k, false)));
    Instruction {
        program_id: sa(ledger::ID),
        accounts,
        data,
    }
}
struct Harness {
    svm: LiteSVM,
    keys: [Keypair; 3],
    roots: std::collections::BTreeMap<Address, Address>,
    nodes: std::cell::RefCell<std::collections::BTreeMap<Address, (Address, Address)>>,
}
impl Harness {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let path = std::env::var_os("CAVALRE_LEDGER_TEST_PROGRAM")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/deploy/cavalre_ledger_solana.so")
            });
        svm.add_program(
            sa(ledger::ID),
            &std::fs::read(path).expect("build sBPF first"),
        )
        .unwrap();
        let keys = std::array::from_fn(|i| Keypair::new_from_array([i as u8 + 1; 32]));
        for k in &keys {
            svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        }
        Self {
            svm,
            keys,
            roots: Default::default(),
            nodes: Default::default(),
        }
    }
    fn key(&self, n: usize) -> Address {
        self.keys[n].pubkey()
    }
    fn external_root(&mut self, mint: Address) -> Address {
        let storage = sa(ledger::ledger_lib::root_storage_address(&ap(SYSTEM), &ap(mint)).0);
        self.roots.insert(mint, storage);
        mint
    }
    fn storage(&self, address: Address) -> Address {
        if let Some(storage) = self.roots.get(&address) {
            return *storage;
        }
        let Some((parent, relative)) = self.nodes.borrow().get(&address).copied() else {
            return address;
        };
        let group = sa(ledger::ledger_lib::account_storage_address(&ap(parent), &ap(relative)).0);
        if self.svm.get_account(&group).is_some_and(|a| {
            a.owner == sa(ledger::ID)
                && ledger::ledger_lib::decode_data(&ap(group), &ap(a.owner), &a.data)
                    .is_ok_and(|r| r.is_container())
        }) {
            group
        } else {
            self.storage(parent)
        }
    }
    fn account(&self, address: &Address) -> Option<Account> {
        let key = if self.nodes.borrow().contains_key(address) {
            self.storage(*address)
        } else {
            *address
        };
        self.svm.get_account(&key)
    }
    fn remember(&self, parent: Address, relative: Address) -> Address {
        let key = child(parent, relative);
        self.nodes.borrow_mut().insert(key, (parent, relative));
        key
    }
    // Client adapter: instructions carry logical identities; account metas carry
    // group containers. Merge duplicate containers and their write privileges.
    fn indexed(&self, mut instruction: Instruction) -> Instruction {
        use anchor_lang::Discriminator;
        let offset = if instruction.program_id == sa(ledger::ID) {
            0
        } else {
            2
        };
        let data = &instruction.data;
        let is = |d: &[u8]| data.starts_with(d);
        if ![
            instruction::AddLedger::DISCRIMINATOR,
            instruction::AddExternalToken::DISCRIMINATOR,
            instruction::AddNativeSol::DISCRIMINATOR,
            instruction::AddSubAccount::DISCRIMINATOR,
            instruction::AddSubAccountGroup::DISCRIMINATOR,
            instruction::AddSubAccountByName::DISCRIMINATOR,
            instruction::AddSubAccountGroupByName::DISCRIMINATOR,
            instruction::RemoveSubAccount::DISCRIMINATOR,
            instruction::RemoveSubAccountGroup::DISCRIMINATOR,
            instruction::Transfer::DISCRIMINATOR,
            instruction::Wrap::DISCRIMINATOR,
            instruction::Unwrap::DISCRIMINATOR,
            instruction::WrapSol::DISCRIMINATOR,
            instruction::UnwrapSol::DISCRIMINATOR,
        ]
        .iter()
        .any(|d| is(d))
        {
            return instruction;
        }
        let key = |at: usize| Address::new_from_array(data[at..at + 32].try_into().unwrap());
        let init = is(instruction::AddLedger::DISCRIMINATOR)
            || is(instruction::AddExternalToken::DISCRIMINATOR)
            || is(instruction::AddNativeSol::DISCRIMINATOR);
        let settlement = is(instruction::Wrap::DISCRIMINATOR)
            || is(instruction::Unwrap::DISCRIMINATOR)
            || is(instruction::WrapSol::DISCRIMINATOR)
            || is(instruction::UnwrapSol::DISCRIMINATOR);
        let mut new_group = None;
        if init {
            let root = if is(instruction::AddExternalToken::DISCRIMINATOR) {
                instruction.accounts[offset + 2].pubkey
            } else if is(instruction::AddNativeSol::DISCRIMINATOR) {
                sa(ledger::ledger_lib::NATIVE_SOL)
            } else {
                instruction.accounts[offset + 2].pubkey
            };
            self.remember(root, sa(SOURCE));
        } else {
            let parent = key(8);
            let named = is(instruction::AddSubAccountByName::DISCRIMINATOR)
                || is(instruction::AddSubAccountGroupByName::DISCRIMINATOR);
            let relative = if named {
                let name = String::deserialize(&mut &data[40..]).unwrap();
                match ledger::ledger_lib::name_to_address(&name) {
                    Ok(key) => sa(key),
                    Err(_) => return instruction,
                }
            } else {
                key(40)
            };
            let logical = self.remember(parent, relative);
            if is(instruction::AddSubAccountGroup::DISCRIMINATOR)
                || is(instruction::AddSubAccountGroupByName::DISCRIMINATOR)
            {
                new_group = Some((
                    logical,
                    sa(ledger::ledger_lib::account_storage_address(&ap(parent), &ap(relative)).0),
                ));
            }
            if is(instruction::Transfer::DISCRIMINATOR) {
                self.remember(key(72), key(104));
            }
            if is(instruction::RemoveSubAccount::DISCRIMINATOR)
                || is(instruction::RemoveSubAccountGroup::DISCRIMINATOR)
            {
                if let Some(record) = self.maybe_record(parent) {
                    if let Some(last) = record.children.checked_sub(1) {
                        let bytes = self.svm.get_account(&self.storage(parent)).unwrap();
                        let relative = sa(ledger::ledger_storage::child(&bytes.data, last)
                            .unwrap()
                            .unwrap());
                        let last = self.remember(parent, relative);
                        if !instruction
                            .accounts
                            .iter()
                            .any(|m| m.pubkey == self.storage(last))
                        {
                            instruction.accounts.push(AccountMeta::new(last, false));
                        }
                    }
                }
            }
            if !settlement {
                let root = instruction.accounts[offset + 2].pubkey;
                if self
                    .maybe_record(root)
                    .is_some_and(|r| r.scope == ap(SYSTEM))
                {
                    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
                        &[b"vault", root.as_ref()],
                        &ledger::ID,
                    )
                    .0);
                    if !instruction.accounts.iter().any(|m| m.pubkey == vault) {
                        instruction
                            .accounts
                            .push(AccountMeta::new_readonly(vault, false));
                    }
                }
            }
        }
        let fixed = offset
            + if init {
                if is(instruction::AddLedger::DISCRIMINATOR) {
                    5
                } else if is(instruction::AddExternalToken::DISCRIMINATOR) {
                    7
                } else {
                    5
                }
            } else if settlement {
                if is(instruction::WrapSol::DISCRIMINATOR)
                    || is(instruction::UnwrapSol::DISCRIMINATOR)
                {
                    7
                } else {
                    9
                }
            } else {
                4
            };
        let mut accounts: Vec<AccountMeta> = Vec::new();
        for (index, mut meta) in instruction.accounts.into_iter().enumerate() {
            if index >= fixed {
                let logical = meta.pubkey;
                meta.pubkey = if let Some((logical, storage)) =
                    new_group.filter(|(logical, _)| *logical == meta.pubkey)
                {
                    let _ = logical;
                    storage
                } else {
                    self.storage(meta.pubkey)
                };
                if let Some(previous) = accounts
                    .iter_mut()
                    .skip(offset)
                    .find(|m| m.pubkey == meta.pubkey && logical != meta.pubkey)
                {
                    previous.is_writable |= meta.is_writable;
                    previous.is_signer |= meta.is_signer;
                    continue;
                }
            }
            accounts.push(meta);
        }
        instruction.accounts = accounts;
        instruction
    }
    fn maybe_record(&self, key: Address) -> Option<Record> {
        let storage = self.storage(key);
        let a = self.svm.get_account(&storage)?;
        if a.owner != sa(ledger::ID) {
            return None;
        }
        let group = ledger::ledger_lib::decode_data(&ap(storage), &ap(a.owner), &a.data).ok()?;
        if group.address() == ap(key) || storage == key {
            return Some(group);
        }
        match ledger::ledger_storage::get(&a.data, &ap(key)).ok()? {
            Some(ledger::ledger_storage::Value::Leaf(r)) => Some(r),
            _ => None,
        }
    }
    fn run(&mut self, n: usize, mut ix: Instruction) -> bool {
        ix = self.indexed(ix);
        self.svm.expire_blockhash();
        for a in &mut ix.accounts {
            if a.pubkey == self.key(n) {
                a.is_signer = true;
            }
        }
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&self.key(n)),
            &[&self.keys[n]],
            self.svm.latest_blockhash(),
        );
        match self.svm.send_transaction(tx) {
            Ok(_) => true,
            Err(e) => {
                eprintln!("{e:?}");
                false
            }
        }
    }
    fn record(&self, key: Address) -> Record {
        self.maybe_record(key)
            .unwrap_or_else(|| panic!("missing logical record {key}"))
    }
    fn base(&self, n: usize, root: Address) -> accounts::LedgerAccounts {
        accounts::LedgerAccounts {
            payer: ap(self.key(n)),
            authority: ap(self.key(n)),
            root: ap(self.storage(root)),
            system_program: ap(SYSTEM),
        }
    }
    fn pack<T: Pack>(&mut self, key: Address, value: T) {
        self.pack_for(key, value, TOKEN);
    }
    fn pack_for<T: Pack>(&mut self, key: Address, value: T, token_program: Address) {
        let mut data = vec![0; T::LEN];
        T::pack(value, &mut data).unwrap();
        self.svm
            .set_account(
                key,
                Account {
                    lamports: self.svm.minimum_balance_for_rent_exemption(T::LEN),
                    data,
                    owner: token_program,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }
    fn token(&self, key: Address) -> u64 {
        TokenAccount::unpack(&self.svm.get_account(&key).unwrap().data[..TokenAccount::LEN])
            .unwrap()
            .amount
    }
    // Published Metaplex prefix as issuer-owned test setup; Ledger must verify
    // both the canonical PDA and owner. Token-2022 inline tests use its real program.
    fn metadata(&mut self, mint: Address, name: &str, symbol: &str) -> Address {
        use anchor_lang::AnchorSerialize;
        let key = sa(ledger::ledger_view::metadata_address(&ap(mint)));
        let mut data = vec![4];
        data.extend_from_slice(self.key(0).as_ref());
        data.extend_from_slice(mint.as_ref());
        name.serialize(&mut data).unwrap();
        symbol.serialize(&mut data).unwrap();
        "https://example.invalid/token"
            .serialize(&mut data)
            .unwrap();
        0u16.serialize(&mut data).unwrap();
        data.extend_from_slice(&[0; 9]);
        self.svm
            .set_account(
                key,
                Account {
                    lamports: self.svm.minimum_balance_for_rent_exemption(data.len()),
                    data,
                    owner: sa(ledger::ledger_view::METAPLEX_METADATA_PROGRAM),
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        key
    }
    fn registration(&self, n: usize, root: Address) -> accounts::RegisterLedger {
        accounts::RegisterLedger {
            payer: ap(self.key(n)),
            authority: ap(self.key(n)),
            root: ap(self.storage(root)),
            system_program: ap(SYSTEM),
            global_root: ledger::ledger_lib::global_root_address().0,
        }
    }
    fn internal(&mut self) -> (Address, Address) {
        self.internal_named("Scale")
    }
    fn internal_named(&mut self, name: &str) -> (Address, Address) {
        let root = sa(ledger::ledger::root_storage_address(&ap(self.key(0)), &ap(self.key(2))).0);
        let source = child(root, sa(SOURCE));
        let mut i = ix(
            self.registration(0, root),
            instruction::AddLedger {
                id: ap(self.key(2)),
                name: name.into(),
                symbol: "UNIT".into(),
                decimals: 6,
            },
            &[source],
        );
        i.accounts[2].is_writable = true;
        assert!(self.run(0, i));
        (root, source)
    }
}

#[test]
fn internal_posting_implicit_receipt_and_atomic_rejection() {
    let mut h = Harness::new();
    let (root, source) = h.internal();
    let receiver = child(root, h.key(1));
    let mut i = ix(
        h.base(0, root),
        instruction::Transfer {
            from_parent: ap(root),
            from: SOURCE,
            to_parent: ap(root),
            to: ap(h.key(1)),
            amount: 100,
        },
        &[source, receiver],
    );
    i.accounts[2].is_writable = true;
    assert!(h.run(0, i));
    assert_eq!(h.record(source).credit, 100);
    assert_eq!(h.record(receiver).debit, 100);
    assert!(!h.record(receiver).registered);
    assert_eq!(h.record(root).debit, 100);
    let before = h.account(&receiver).unwrap();
    let i = ix(
        h.base(1, root),
        instruction::Transfer {
            from_parent: ap(root),
            from: ap(h.key(1)),
            to_parent: ap(root),
            to: SOURCE,
            amount: 1,
        },
        &[receiver, source],
    );
    assert!(!h.run(1, i));
    assert_eq!(h.account(&receiver).unwrap(), before);
    let mut i = ix(
        h.base(0, root),
        instruction::Transfer {
            from_parent: ap(root),
            from: ap(h.key(1)),
            to_parent: ap(root),
            to: SOURCE,
            amount: 101,
        },
        &[receiver, source],
    );
    i.accounts[2].is_writable = true;
    assert!(!h.run(0, i));
    assert_eq!(h.record(root).debit, 100);
}
#[test]
fn account_lifecycle_and_registered_only_parent() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let group = child(root, h.key(0));
    let mut i = ix(
        h.base(0, root),
        instruction::AddSubAccountGroup {
            parent: ap(root),
            relative: ap(h.key(0)),
            name: "Group".into(),
            credit: false,
            implicit_allowed: false,
        },
        &[group],
    );
    i.accounts[2].is_writable = true;
    assert!(h.run(0, i));
    let leaf = child(group, h.key(1));
    let source = child(root, sa(SOURCE));
    let mut i = ix(
        h.base(0, root),
        instruction::Transfer {
            from_parent: ap(root),
            from: SOURCE,
            to_parent: ap(group),
            to: ap(h.key(1)),
            amount: 1,
        },
        &[source, group, leaf],
    );
    i.accounts[2].is_writable = true;
    assert!(!h.run(0, i));
    assert!(h.maybe_record(leaf).is_none());
    let i = ix(
        h.base(0, root),
        instruction::AddSubAccount {
            parent: ap(group),
            relative: ap(h.key(1)),
            name: "Leaf".into(),
            credit: false,
        },
        &[group, leaf],
    );
    assert!(h.run(0, i));
    assert_eq!(h.record(group).children, 1);
    let mut i = ix(
        h.base(0, root),
        instruction::RemoveSubAccountGroup {
            parent: ap(root),
            relative: ap(h.key(0)),
        },
        &[group],
    );
    i.accounts[2].is_writable = true;
    assert!(!h.run(0, i));
    let i = ix(
        h.base(0, root),
        instruction::RemoveSubAccount {
            parent: ap(group),
            relative: ap(h.key(1)),
        },
        &[group, leaf],
    );
    assert!(h.run(0, i));
    assert!(!h.record(leaf).registered);
    assert_eq!(h.record(group).children, 0);
}
#[test]
fn external_token_deposit_transfer_withdraw_and_isolation() {
    let mut h = Harness::new();
    let mint = Address::new_from_array([21; 32]);
    let wallet = Address::new_from_array([22; 32]);
    h.pack(
        mint,
        Mint {
            mint_authority: COption::None,
            supply: 1000,
            decimals: 6,
            is_initialized: true,
            freeze_authority: COption::None,
        },
    );
    h.pack(
        wallet,
        TokenAccount {
            mint,
            owner: h.key(0),
            amount: 1000,
            delegate: COption::None,
            state: AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        },
    );
    let metadata = h.metadata(mint, "Token", "TOK");
    let root = h.external_root(mint);
    let source = child(root, sa(SOURCE));
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", h.storage(root).as_ref()],
        &ledger::ID,
    )
    .0);
    assert!(h.run(
        0,
        ix(
            accounts::RegisterToken {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                root: ap(h.storage(root)),
                mint: ap(mint),
                vault: ap(vault),
                token_program: ap(TOKEN),
                system_program: ap(SYSTEM)
            },
            instruction::AddExternalToken {},
            &[source, metadata]
        )
    ));
    let group = child(root, h.key(0));
    let mut i = ix(
        h.base(0, root),
        instruction::AddSubAccountGroup {
            parent: ap(root),
            relative: ap(h.key(0)),
            name: "App".into(),
            credit: false,
            implicit_allowed: true,
        },
        &[group],
    );
    i.accounts[2].is_writable = true;
    assert!(h.run(0, i));
    let a = child(group, h.key(1));
    let b = child(group, h.key(2));
    let move_accounts = |h: &Harness, n| accounts::MoveTokens {
        payer: ap(h.key(n)),
        authority: ap(h.key(n)),
        funding_authority: ap(h.key(n)),
        root: ap(h.storage(root)),
        mint: ap(mint),
        vault: ap(vault),
        wallet: ap(wallet),
        token_program: ap(TOKEN),
        system_program: ap(SYSTEM),
    };
    assert!(h.run(
        0,
        ix(
            move_accounts(&h, 0),
            instruction::Wrap {
                parent: ap(group),
                relative: ap(h.key(1)),
                amount: 100
            },
            &[source, group, a]
        )
    ));
    assert_eq!(h.token(vault), 100);
    assert_eq!(h.record(source).credit, 100);
    assert_eq!(h.record(a).debit, 100);
    assert!(!h.record(a).registered);
    let i = ix(
        h.base(0, root),
        instruction::Transfer {
            from_parent: ap(group),
            from: ap(h.key(1)),
            to_parent: ap(group),
            to: ap(h.key(2)),
            amount: 40,
        },
        &[group, a, b],
    );
    assert!(h.run(0, i));
    assert_eq!(h.record(b).debit, 40);
    assert_eq!(h.record(group).debit, 100);
    let i = ix(
        h.base(1, root),
        instruction::Transfer {
            from_parent: ap(group),
            from: ap(h.key(1)),
            to_parent: ap(group),
            to: ap(h.key(2)),
            amount: 1,
        },
        &[group, a, b],
    );
    assert!(!h.run(1, i));
    assert!(!h.run(
        1,
        ix(
            move_accounts(&h, 1),
            instruction::Unwrap {
                parent: ap(group),
                relative: ap(h.key(2)),
                amount: 1
            },
            &[source, group, b]
        )
    ));
    assert!(h.run(
        0,
        ix(
            move_accounts(&h, 0),
            instruction::Unwrap {
                parent: ap(group),
                relative: ap(h.key(2)),
                amount: 40
            },
            &[source, group, b]
        )
    ));
    assert_eq!(h.token(vault), 60);
    assert_eq!(h.token(wallet), 940);
    assert_eq!(h.record(root).debit, 60);
    assert_eq!(h.record(source).credit, 60);
    let old = h.account(&a).unwrap();
    assert!(!h.run(
        0,
        ix(
            move_accounts(&h, 0),
            instruction::Unwrap {
                parent: ap(group),
                relative: ap(h.key(1)),
                amount: 61
            },
            &[source, group, a]
        )
    ));
    assert_eq!(h.account(&a).unwrap(), old);
    // Token transfer succeeds before the late read-only ancestor write fails.
    // The runtime must undo token movement, rent allocation and ledger writes.
    let before_vault = h.token(vault);
    let before_wallet = h.token(wallet);
    let mut late_failure = ix(
        move_accounts(&h, 0),
        instruction::Wrap {
            parent: ap(group),
            relative: ap(h.key(1)),
            amount: 1,
        },
        &[source, group, a],
    );
    late_failure = h.indexed(late_failure);
    late_failure
        .accounts
        .iter_mut()
        .find(|m| m.pubkey == h.storage(group))
        .unwrap()
        .is_writable = false;
    assert!(!h.run(0, late_failure));
    assert_eq!(h.token(vault), before_vault);
    assert_eq!(h.token(wallet), before_wallet);
    assert_eq!(h.account(&a).unwrap(), old);
    // A vault that can pay this amount but cannot cover total claims must reject.
    let mut native = TokenAccount::unpack(&h.account(&vault).unwrap().data).unwrap();
    native.amount = 10;
    h.pack(vault, native);
    assert!(!h.run(
        0,
        ix(
            move_accounts(&h, 0),
            instruction::Unwrap {
                parent: ap(group),
                relative: ap(h.key(1)),
                amount: 1,
            },
            &[source, group, a]
        )
    ));
    assert_eq!(h.token(vault), 10);
    assert_eq!(h.record(a).debit, 60);
    // Read actual persisted program records even while custody cannot satisfy a
    // withdrawal. Inspection has no signer, token settlement or mutation gate.
    let mut reader = ledger::ledger_view::Reader::new();
    for key in [root, group] {
        let snapshot = h.account(&h.storage(key)).unwrap();
        reader
            .insert(ap(h.storage(key)), &ap(snapshot.owner), &snapshot.data)
            .unwrap();
    }
    assert_eq!(
        reader
            .balance_of(&ap(root), &ap(group), &ap(h.key(1)))
            .unwrap(),
        60
    );
    assert_eq!(reader.total_supply(&ap(root)).unwrap(), 60);
}

#[test]
fn application_pda_can_create_its_branch_but_another_application_cannot() {
    let mut h = Harness::new();
    let mint = Address::new_from_array([31; 32]);
    h.pack(
        mint,
        Mint {
            mint_authority: COption::None,
            supply: 0,
            decimals: 6,
            is_initialized: true,
            freeze_authority: COption::None,
        },
    );
    let metadata = h.metadata(mint, "Token", "TOK");
    let root = h.external_root(mint);
    let source = child(root, sa(SOURCE));
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", h.storage(root).as_ref()],
        &ledger::ID,
    )
    .0);
    assert!(h.run(
        0,
        ix(
            accounts::RegisterToken {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                root: ap(h.storage(root)),
                mint: ap(mint),
                vault: ap(vault),
                token_program: ap(TOKEN),
                system_program: ap(SYSTEM)
            },
            instruction::AddExternalToken {},
            &[source, metadata]
        )
    ));
    let app = Address::new_from_array([41; 32]);
    let other = Address::new_from_array([42; 32]);
    let binary = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap();
    for program in [app, other] {
        h.svm.add_program(program, &binary).unwrap();
    }
    let authority = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"app", h.key(0).as_ref()],
        &ap(app),
    )
    .0);
    let group = child(root, authority);
    let mut base = h.base(0, root);
    base.authority = ap(authority);
    let mut inner = ix(
        base,
        instruction::AddSubAccountGroup {
            parent: ap(root),
            relative: ap(authority),
            name: "Application".into(),
            credit: false,
            implicit_allowed: true,
        },
        &[group],
    );
    for meta in &mut inner.accounts {
        if meta.pubkey == authority {
            meta.is_signer = false;
        }
    }
    let mut accounts = vec![
        AccountMeta::new_readonly(h.key(0), true),
        AccountMeta::new_readonly(sa(ledger::ID), false),
    ];
    accounts.extend(inner.accounts);
    let proxy = Instruction {
        program_id: other,
        accounts: accounts.clone(),
        data: inner.data.clone(),
    };
    assert!(!h.run(0, proxy));
    assert!(h.maybe_record(group).is_none());
    assert!(h.run(
        0,
        Instruction {
            program_id: app,
            accounts,
            data: inner.data
        }
    ));
    assert!(h.record(group).registered);
    assert_eq!(h.record(group).relative, ap(authority));
}
