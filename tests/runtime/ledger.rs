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
        Self {
            svm,
            keys,
            roots: Default::default(),
        }
    }
    fn key(&self, n: usize) -> Address {
        self.keys[n].pubkey()
    }
    fn external_root(&mut self, mint: Address) -> Address {
        let storage = sa(ledger::ledger_lib::ledger_pda(&ap(SYSTEM), &ap(mint)).0);
        self.roots.insert(storage, storage);
        storage
    }
    fn storage(&self, address: Address) -> Address {
        self.roots.get(&address).copied().unwrap_or(address)
    }
    // Client preparation for structural changes: inline child vectors require
    // parent writes, and optional metadata uses its independent PDA namespace.
    fn indexed(&self, mut instruction: Instruction) -> Instruction {
        use anchor_lang::Discriminator;
        use ledger::ledger_lib::{
            decode_data, global_root_address, ledger_relative, metadata_address,
        };
        let offset = if instruction.program_id == sa(ledger::ID) {
            0
        } else {
            2
        };
        let data = &instruction.data;
        if data.starts_with(ledger::ledger_transfer::TRANSFER) {
            let root = instruction.accounts[offset + 1].pubkey;
            if let Some(record) = self
                .svm
                .get_account(&root)
                .and_then(|a| decode_data(&ap(root), &ap(a.owner), &a.data).ok())
            {
                if let Some(config) = record.ledger.filter(|c| c.authority == ap(SYSTEM)) {
                    let vault = sa(config.vault);
                    if !instruction.accounts.iter().any(|a| a.pubkey == vault) {
                        instruction
                            .accounts
                            .push(AccountMeta::new_readonly(vault, false));
                    }
                }
            }
            return instruction;
        }
        let get = |key: Address| {
            self.svm
                .get_account(&key)
                .and_then(|a| decode_data(&ap(key), &ap(a.owner), &a.data).ok())
        };
        let mut extra = Vec::new();
        let add_ledger = data.starts_with(instruction::AddLedger::DISCRIMINATOR);
        let add_token = data.starts_with(instruction::AddExternalToken::DISCRIMINATOR);
        let add_sol = data.starts_with(instruction::AddNativeSol::DISCRIMINATOR);
        let add = data.starts_with(instruction::AddSubAccount::DISCRIMINATOR)
            || data.starts_with(instruction::AddSubAccountGroup::DISCRIMINATOR);
        let named = data.starts_with(instruction::AddSubAccountByName::DISCRIMINATOR)
            || data.starts_with(instruction::AddSubAccountGroupByName::DISCRIMINATOR);
        let remove = data.starts_with(instruction::RemoveSubAccount::DISCRIMINATOR)
            || data.starts_with(instruction::RemoveSubAccountGroup::DISCRIMINATOR);
        if add_ledger || add_token || add_sol {
            let root = instruction.accounts[offset + if add_ledger { 2 } else { 1 }].pubkey;
            let relative = if add_ledger {
                let id =
                    anchor_lang::prelude::Pubkey::new_from_array(data[8..40].try_into().unwrap());
                ledger_relative(&ap(instruction.accounts[offset + 1].pubkey), &id)
            } else if add_token {
                ap(instruction.accounts[offset + 2].pubkey)
            } else {
                ledger::ledger_lib::NATIVE_SOL
            };
            extra.push(sa(metadata_address(&global_root_address().0, &relative).0));
            extra.push(sa(metadata_address(&ap(root), &SOURCE).0));
        } else if add || named || remove {
            let parent = Address::new_from_array(data[8..40].try_into().unwrap());
            let relative = if named {
                let name = String::deserialize(&mut &data[40..]).unwrap();
                let Ok(relative) = ledger::ledger_lib::name_to_address(&name) else {
                    return instruction;
                };
                sa(relative)
            } else {
                Address::new_from_array(data[40..72].try_into().unwrap())
            };
            extra.push(sa(metadata_address(&ap(parent), &ap(relative)).0));
            if remove {
                if let Some(record) = get(parent) {
                    if let Some(last) = record.children.last() {
                        extra.push(child(parent, sa(*last)));
                    }
                }
            }
        }
        if !(add_ledger || add_token || add_sol) && instruction.accounts.len() >= offset + 4 {
            let root = instruction.accounts[offset + 2].pubkey;
            if let Some(record) = get(root) {
                if let Some(config) = record.ledger.filter(|c| c.authority == ap(SYSTEM)) {
                    let vault = sa(config.vault);
                    if !instruction.accounts.iter().any(|a| a.pubkey == vault) {
                        instruction
                            .accounts
                            .push(AccountMeta::new_readonly(vault, false));
                    }
                }
            }
        }
        for key in extra {
            if !instruction.accounts.iter().any(|a| a.pubkey == key) {
                instruction.accounts.push(AccountMeta::new(key, false));
            }
        }
        instruction
    }
    // Client composition for acceptance workflows. Raw-instruction tests bypass
    // this helper to prove Transfer never allocates and creation is idempotent.
    fn prepared(&self, instruction: Instruction) -> Vec<Instruction> {
        if !instruction
            .data
            .starts_with(ledger::ledger_transfer::TRANSFER)
            || instruction.data.len() != 154
        {
            return vec![instruction];
        }
        let offset = usize::from(instruction.program_id != sa(ledger::ID)) * 2;
        let root = instruction.accounts[offset + 1].pubkey;
        let authority = instruction.accounts[offset].pubkey;
        let mut result = Vec::new();
        let mut created = Vec::new();
        // Creation is deliberately explicit even for zero-valued transfers:
        // the sole transfer instruction always requires valid stored endpoints.
        for (role, at) in [(2, 8), (3, 72)] {
            let key = instruction.accounts[offset + role].pubkey;
            if created.contains(&key) || self.svm.get_account(&key).is_some() {
                continue;
            }
            let parent = ap(Address::new_from_array(
                instruction.data[at..at + 32].try_into().unwrap(),
            ));
            let relative = ap(Address::new_from_array(
                instruction.data[at + 32..at + 64].try_into().unwrap(),
            ));
            let remaining: Vec<_> = instruction.accounts[offset + 4..]
                .iter()
                .map(|m| anchor_lang::solana_program::instruction::AccountMeta {
                    pubkey: ap(m.pubkey),
                    is_writable: m.is_writable,
                    is_signer: m.is_signer,
                })
                .collect();
            let call = ledger::ledger_transfer::create_idempotent_instruction(
                ap(self.key(0)),
                ap(authority),
                ap(root),
                ledger::ledger_lib::Child { parent, relative },
                &remaining,
            );
            let mut call = Instruction {
                program_id: sa(call.program_id),
                data: call.data,
                accounts: call
                    .accounts
                    .into_iter()
                    .map(|m| AccountMeta {
                        pubkey: sa(m.pubkey),
                        is_writable: m.is_writable,
                        is_signer: m.is_signer,
                    })
                    .collect(),
            };
            // Do not grant permissions missing from the requested composition.
            for meta in &mut call.accounts {
                if let Some(original) = instruction
                    .accounts
                    .iter()
                    .find(|m| m.pubkey == meta.pubkey)
                {
                    meta.is_writable &= original.is_writable;
                }
            }
            if offset != 0 {
                for meta in &mut call.accounts {
                    if meta.pubkey == authority {
                        meta.is_signer = false;
                    }
                }
                let mut accounts = instruction.accounts[..offset].to_vec();
                accounts.extend(call.accounts);
                call.accounts = accounts;
                call.program_id = instruction.program_id;
            }
            result.push(call);
            created.push(key);
        }
        result.push(instruction);
        result
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
            &self.prepared(ix),
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
        let a = self.svm.get_account(&self.storage(key)).unwrap();
        ledger::ledger_lib::decode_data(&ap(self.storage(key)), &ap(a.owner), &a.data).unwrap()
    }
    fn metadata_key(&self, key: Address) -> Address {
        let record = self.record(key);
        let relative = if let Some(config) = &record.ledger {
            ledger::ledger_lib::ledger_relative(&config.authority, &config.identifier)
        } else {
            self.record(sa(record.parent)).children[(record.child_index - 1) as usize]
        };
        sa(ledger::ledger_lib::metadata_address(&record.parent, &relative).0)
    }
    fn insert_labels(&self, reader: &mut ledger::ledger_view::Reader, key: Address) {
        let record = self.record(key);
        let relative = if let Some(config) = &record.ledger {
            ledger::ledger_lib::ledger_relative(&config.authority, &config.identifier)
        } else {
            self.record(sa(record.parent)).children[(record.child_index - 1) as usize]
        };
        let meta_key = self.metadata_key(key);
        let meta = self.svm.get_account(&meta_key).unwrap();
        reader
            .insert_metadata(
                ap(meta_key),
                &ap(meta.owner),
                &meta.data,
                &record.parent,
                &relative,
            )
            .unwrap();
    }
    fn labels(&self, key: Address) -> ledger::ledger_lib::Metadata {
        let record = self.record(key);
        let relative = if let Some(config) = &record.ledger {
            ledger::ledger_lib::ledger_relative(&config.authority, &config.identifier)
        } else {
            self.record(sa(record.parent)).children[(record.child_index - 1) as usize]
        };
        let metadata = sa(ledger::ledger_lib::metadata_address(&record.parent, &relative).0);
        self.svm
            .get_account(&metadata)
            .map(|a| ledger::ledger_storage::decode_metadata(&a.data).unwrap())
            .unwrap_or(ledger::ledger_lib::Metadata {
                bump: 0,
                decimals: 0,
                name: String::new(),
                symbol: String::new(),
            })
    }
    fn base(&self, n: usize, root: Address) -> accounts::LedgerAccounts {
        accounts::LedgerAccounts {
            payer: ap(self.key(n)),
            authority: ap(self.key(n)),
            ledger: ap(self.storage(root)),
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
            ledger: ap(self.storage(root)),
            system_program: ap(SYSTEM),
            global_root: ledger::ledger_lib::global_root_address().0,
        }
    }
    fn internal(&mut self) -> (Address, Address) {
        self.internal_named("Scale")
    }
    fn internal_named(&mut self, name: &str) -> (Address, Address) {
        let root = sa(ledger::ledger::ledger_pda(&ap(self.key(0)), &ap(self.key(2))).0);
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
    let mut i = acceptance::transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (root, h.key(1)),
        100,
        &[],
    );
    i.accounts[2].is_writable = true;
    assert!(h.run(0, i));
    assert_eq!(h.record(source).credit, 100);
    assert_eq!(h.record(receiver).debit, 100);
    assert!(h.record(receiver).child_index > 0);
    assert_eq!(h.record(root).debit, 100);
    let before = h.svm.get_account(&receiver).unwrap();
    let i = acceptance::transfer(
        &h,
        root,
        h.key(1),
        (root, h.key(1)),
        (root, sa(SOURCE)),
        1,
        &[],
    );
    assert!(!h.run(1, i));
    assert_eq!(h.svm.get_account(&receiver).unwrap(), before);
    let mut i = acceptance::transfer(
        &h,
        root,
        h.key(0),
        (root, h.key(1)),
        (root, sa(SOURCE)),
        101,
        &[],
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
        },
        &[group],
    );
    i.accounts[2].is_writable = true;
    assert!(h.run(0, i));
    let leaf = child(group, h.key(1));
    let mut i = acceptance::transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (group, h.key(1)),
        0,
        &[],
    );
    i.accounts[2].is_writable = true;
    assert!(acceptance::run_raw(&mut h, &[0], i).is_err());
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
    assert_eq!(h.record(group).children.len(), 1);
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
    assert!(h.svm.get_account(&leaf).is_none());
    assert_eq!(h.record(group).children.len(), 0);
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
                ledger: ap(h.storage(root)),
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
        ledger: ap(h.storage(root)),
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
    assert!(h.record(a).child_index > 0);
    let i = acceptance::transfer(
        &h,
        root,
        h.key(0),
        (group, h.key(1)),
        (group, h.key(2)),
        40,
        &[],
    );
    assert!(h.run(0, i));
    assert_eq!(h.record(b).debit, 40);
    assert_eq!(h.record(group).debit, 100);
    let i = acceptance::transfer(
        &h,
        root,
        h.key(1),
        (group, h.key(1)),
        (group, h.key(2)),
        1,
        &[],
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
        let snapshot = h.svm.get_account(&h.storage(key)).unwrap();
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
                ledger: ap(h.storage(root)),
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
    assert!(h.record(group).child_index > 0);
    assert_eq!(
        h.record(root).children[(h.record(group).child_index - 1) as usize],
        ap(authority)
    );
}
