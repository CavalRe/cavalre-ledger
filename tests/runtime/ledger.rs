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
    sa(ledger::ledger_lib::to_address(&ledger::ID, &ap(parent), &ap(relative)).0)
}
fn ix(a: impl ToAccountMetas, d: impl InstructionData, rest: &[Address]) -> Instruction {
    let mut accounts: Vec<_> = a
        .to_account_metas(None)
        .into_iter()
        .map(|m| AccountMeta {
            pubkey: sa(m.pubkey),
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        })
        .collect();
    accounts.extend(rest.iter().map(|k| AccountMeta::new(*k, false)));
    Instruction {
        program_id: sa(ledger::ID),
        accounts,
        data: d.data(),
    }
}
struct Harness {
    svm: LiteSVM,
    keys: [Keypair; 3],
}
impl Harness {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_solana.so");
        svm.add_program(
            sa(ledger::ID),
            &std::fs::read(path).expect("build sBPF first"),
        )
        .unwrap();
        let keys = std::array::from_fn(|i| Keypair::new_from_array([i as u8 + 1; 32]));
        for k in &keys {
            svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        }
        Self { svm, keys }
    }
    fn key(&self, n: usize) -> Address {
        self.keys[n].pubkey()
    }
    fn run(&mut self, n: usize, mut ix: Instruction) -> bool {
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
        let a = self.svm.get_account(&key).unwrap();
        Record::deserialize(&mut &a.data[8..]).unwrap()
    }
    fn base(&self, n: usize, root: Address) -> accounts::LedgerAccounts {
        accounts::LedgerAccounts {
            payer: ap(self.key(n)),
            authority: ap(self.key(n)),
            root: ap(root),
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
    fn registration(&self, n: usize, root: Address) -> accounts::RegisterLedger {
        accounts::RegisterLedger {
            payer: ap(self.key(n)),
            authority: ap(self.key(n)),
            root: ap(root),
            system_program: ap(SYSTEM),
            global_root: ledger::ledger_lib::global_root_address().0,
        }
    }
    fn internal(&mut self) -> (Address, Address) {
        self.internal_named("Scale")
    }
    fn internal_named(&mut self, name: &str) -> (Address, Address) {
        let root = sa(ledger::ledger::root_address(&ap(self.key(0)), &ap(self.key(2))).0);
        let source = child(root, sa(SOURCE));
        let mut i = ix(
            self.registration(0, root),
            instruction::AddLedger {
                id: ap(self.key(2)),
                name: name.into(),
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
    let before = h.svm.get_account(&receiver).unwrap();
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
    assert_eq!(h.svm.get_account(&receiver).unwrap(), before);
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
    assert!(h.svm.get_account(&leaf).is_none());
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
    let root = sa(ledger::ledger::root_address(&ap(SYSTEM), &ap(mint)).0);
    let source = child(root, sa(SOURCE));
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", root.as_ref()],
        &ledger::ID,
    )
    .0);
    assert!(h.run(
        0,
        ix(
            accounts::RegisterToken {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                root: ap(root),
                mint: ap(mint),
                vault: ap(vault),
                token_program: ap(TOKEN),
                system_program: ap(SYSTEM)
            },
            instruction::AddExternalToken {
                name: "USDC".into()
            },
            &[source]
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
        root: ap(root),
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
    let old = h.svm.get_account(&a).unwrap();
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
    assert_eq!(h.svm.get_account(&a).unwrap(), old);
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
    late_failure
        .accounts
        .iter_mut()
        .find(|m| m.pubkey == group)
        .unwrap()
        .is_writable = false;
    assert!(!h.run(0, late_failure));
    assert_eq!(h.token(vault), before_vault);
    assert_eq!(h.token(wallet), before_wallet);
    assert_eq!(h.svm.get_account(&a).unwrap(), old);
    // A vault that can pay this amount but cannot cover total claims must reject.
    let mut native = TokenAccount::unpack(&h.svm.get_account(&vault).unwrap().data).unwrap();
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
    for key in [root, source, group, a, b] {
        let snapshot = h.svm.get_account(&key).unwrap();
        reader
            .insert(ap(key), &ap(snapshot.owner), &snapshot.data)
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
    let root = sa(ledger::ledger::root_address(&ap(SYSTEM), &ap(mint)).0);
    let source = child(root, sa(SOURCE));
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", root.as_ref()],
        &ledger::ID,
    )
    .0);
    assert!(h.run(
        0,
        ix(
            accounts::RegisterToken {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                root: ap(root),
                mint: ap(mint),
                vault: ap(vault),
                token_program: ap(TOKEN),
                system_program: ap(SYSTEM)
            },
            instruction::AddExternalToken {
                name: "Token".into()
            },
            &[source]
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
    assert!(h.svm.get_account(&group).is_none());
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
