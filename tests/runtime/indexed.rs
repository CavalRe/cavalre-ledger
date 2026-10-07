use anchor_lang::prelude::Pubkey;
use cavalre_ledger_indexed::{
    self as indexed, storage, AccountRef, Command, Header, Record, HEADER_LEN, RECORD_LEN,
};
use litesvm::{types::TransactionMetadata, LiteSVM};
use solana_account::Account;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;

fn sa(key: Pubkey) -> Address {
    Address::new_from_array(key.to_bytes())
}
fn ap(key: Address) -> Pubkey {
    Pubkey::new_from_array(key.to_bytes())
}
fn relative(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}
struct Harness {
    svm: LiteSVM,
    authority: Keypair,
    containers: Vec<Keypair>,
}
impl Harness {
    fn new(capacity: usize) -> Self {
        let mut svm = LiteSVM::new();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_indexed.so");
        svm.add_program(
            sa(indexed::ID),
            &std::fs::read(path).expect("build indexed sBPF first"),
        )
        .unwrap();
        let authority = Keypair::new_from_array([111; 32]);
        svm.airdrop(&authority.pubkey(), 10_000_000_000).unwrap();
        let mut h = Self {
            svm,
            authority,
            containers: Vec::new(),
        };
        h.new_container(capacity, true);
        h
    }
    fn key(&self, n: usize) -> Pubkey {
        ap(self.containers[n].pubkey())
    }
    fn reference(&self, n: usize, index: u32) -> AccountRef {
        AccountRef::new(self.key(n), index)
    }
    fn new_container(&mut self, capacity: usize, root: bool) -> usize {
        let n = self.containers.len();
        let key = Keypair::new_from_array([120 + n as u8; 32]);
        // Simulate System allocation; initialization still requires the new
        // container's signature, program ownership and all-zero bytes.
        self.svm
            .set_account(
                key.pubkey(),
                Account {
                    lamports: 100_000_000,
                    data: vec![0; HEADER_LEN + capacity * RECORD_LEN],
                    owner: sa(indexed::ID),
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        self.containers.push(key);
        let command = if root {
            Command::InitializeLedger
        } else {
            Command::InitializeContainer {
                ledger: self.reference(0, 0),
            }
        };
        let auxiliary = if root {
            indexed::index_address(&self.key(n), self.reference(n, 0), &indexed::SOURCE).0
        } else {
            self.key(0)
        };
        let mut call = self.structural(n, command, auxiliary, &[]);
        call.accounts[2].is_signer = true;
        self.send(vec![call], Some(n)).unwrap();
        n
    }
    fn structural(
        &self,
        n: usize,
        command: Command,
        auxiliary: Pubkey,
        extra: &[Pubkey],
    ) -> Instruction {
        let mut accounts = vec![
            AccountMeta::new(self.authority.pubkey(), true),
            AccountMeta::new_readonly(self.authority.pubkey(), true),
            AccountMeta::new(sa(self.key(n)), false),
            AccountMeta::new_readonly(Address::default(), false),
            AccountMeta::new(sa(auxiliary), false),
        ];
        accounts.extend(extra.iter().map(|k| AccountMeta::new(sa(*k), false)));
        Instruction {
            program_id: sa(indexed::ID),
            accounts,
            data: indexed::encode(&command).unwrap(),
        }
    }
    fn send(
        &mut self,
        calls: Vec<Instruction>,
        container_signer: Option<usize>,
    ) -> Result<TransactionMetadata, String> {
        self.svm.expire_blockhash();
        let mut signers = vec![&self.authority];
        if let Some(n) = container_signer {
            signers.push(&self.containers[n]);
        }
        let tx = Transaction::new_signed_with_payer(
            &calls,
            Some(&self.authority.pubkey()),
            &signers,
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx).map_err(|e| format!("{e:?}"))
    }
    fn add_ix(&self, n: usize, parent: AccountRef, relative: Pubkey, kind: u8) -> Instruction {
        self.structural(
            n,
            Command::Register {
                parent,
                relative,
                kind,
            },
            indexed::index_address(&self.key(n), parent, &relative).0,
            &[parent.container],
        )
    }
    fn add(&mut self, n: usize, parent: AccountRef, relative: Pubkey, kind: u8) -> AccountRef {
        let index = Header::read(&self.data(n)).unwrap().len;
        self.send(vec![self.add_ix(n, parent, relative, kind)], None)
            .unwrap();
        let lookup = self
            .svm
            .get_account(&sa(
                indexed::index_address(&self.key(n), parent, &relative).0
            ))
            .unwrap();
        assert_eq!(lookup.owner, sa(indexed::ID));
        assert_eq!(lookup.data, index.to_le_bytes());
        self.reference(n, index)
    }
    fn transfer_ix(&self, from: AccountRef, to: AccountRef, amount: u128) -> Instruction {
        let mut accounts = vec![AccountMeta::new_readonly(self.authority.pubkey(), true)];
        accounts.extend(
            self.containers
                .iter()
                .map(|k| AccountMeta::new(k.pubkey(), false)),
        );
        Instruction {
            program_id: sa(indexed::ID),
            accounts,
            data: indexed::encode(&Command::Transfer { from, to, amount }).unwrap(),
        }
    }
    fn transfer(&mut self, from: AccountRef, to: AccountRef, amount: u128) -> TransactionMetadata {
        self.send(vec![self.transfer_ix(from, to, amount)], None)
            .unwrap()
    }
    fn remove_ix(&self, n: usize, index: u32, relative: Pubkey) -> Instruction {
        let record = self.record(self.reference(n, index));
        self.structural(
            n,
            Command::Remove { index, relative },
            indexed::index_address(&self.key(n), record.parent, &relative).0,
            &[record.parent.container],
        )
    }
    fn metadata_ix(&self, n: usize, index: u32, name: &str) -> Instruction {
        self.structural(
            n,
            Command::Metadata {
                index,
                decimals: 18,
                name: name.into(),
                symbol: "TEST".into(),
            },
            indexed::metadata_address(self.reference(n, index)).0,
            &[],
        )
    }
    fn data(&self, n: usize) -> Vec<u8> {
        self.svm.get_account(&sa(self.key(n))).unwrap().data
    }
    fn snapshot(&self) -> Vec<Vec<u8>> {
        (0..self.containers.len()).map(|n| self.data(n)).collect()
    }
    fn record(&self, r: AccountRef) -> Record {
        storage::read_record(
            &self.svm.get_account(&sa(r.container)).unwrap().data,
            r.index,
        )
        .unwrap()
    }
}
#[test]
fn fixed_records_cross_container_walk_and_cu() {
    let mut h = Harness::new(16);
    let root = h.reference(0, 0);
    let source = h.reference(0, 1);
    let group = h.add(0, root, relative(10), 0);
    let alice = h.add(0, group, relative(11), 2);
    let bob = h.add(0, group, relative(12), 2);
    h.new_container(16, false);
    let group2 = h.add(1, root, relative(20), 0);
    let nested = h.add(1, group2, relative(21), 0);
    let carol = h.add(1, nested, relative(11), 2); // same relative, different parent
    let credit = h.add(0, group, relative(13), 3);
    assert_eq!(h.record(carol).custodian, group2);
    assert_eq!(h.record(alice).custodian, group);
    let bootstrap = h.transfer(source, alice, 100).compute_units_consumed;
    assert_eq!((h.record(root).debit, h.record(root).credit), (100, 100));
    let parent_before = h.record(group);
    let result = h.transfer(alice, bob, 7);
    let siblings = result.compute_units_consumed;
    assert_eq!(
        result
            .logs
            .iter()
            .filter(|l| l.starts_with("Program data:"))
            .count(),
        2
    );
    assert_eq!((h.record(alice).debit, h.record(bob).debit), (93, 7));
    assert_eq!(h.record(group), parent_before);
    let branches = h.transfer(alice, carol, 13).compute_units_consumed;
    for (r, debit) in [
        (root, 100),
        (group, 87),
        (alice, 80),
        (bob, 7),
        (group2, 13),
        (nested, 13),
        (carol, 13),
    ] {
        assert_eq!(h.record(r).debit, debit);
    }
    let opposite = h.transfer(credit, bob, 5).compute_units_consumed;
    assert_eq!(h.record(credit).credit, 5);
    assert_eq!(h.record(bob).debit, 12);
    assert_eq!((h.record(group).debit, h.record(group).credit), (92, 5));
    assert_eq!((h.record(root).debit, h.record(root).credit), (105, 105));
    let before = h.snapshot();
    h.transfer(alice, alice, 1);
    h.transfer(alice, bob, 0);
    assert_eq!(h.snapshot(), before);
    // No index or metadata PDA occurs in any transfer account list.
    let mut costs = [
        ("bootstrap", bootstrap),
        ("siblings", siblings),
        ("cross_container_unequal_depth", branches),
        ("opposite_polarity", opposite),
    ];
    costs.sort_by_key(|x| std::cmp::Reverse(x.1));
    for (name, cu) in costs {
        eprintln!("INDEXED_INTERNAL_CU {name}={cu}");
    }
}
#[test]
fn index_metadata_removal_and_stable_indices() {
    let mut h = Harness::new(12);
    let root = h.reference(0, 0);
    let group = h.add(0, root, relative(10), 0);
    let alice = h.add(0, group, relative(11), 2);
    let bob = h.add(0, group, relative(12), 2);
    let before = h.snapshot();
    let metadata = h.metadata_ix(0, alice.index, "Alice");
    h.send(vec![metadata.clone()], None).unwrap();
    assert_eq!(h.snapshot(), before);
    let meta = h
        .svm
        .get_account(&sa(indexed::metadata_address(alice).0))
        .unwrap();
    assert_eq!(
        meta.data,
        indexed::encode(&(18u8, String::from("Alice"), String::from("TEST"))).unwrap()
    );
    h.send(vec![metadata], None).unwrap();
    assert!(h
        .send(vec![h.metadata_ix(0, alice.index, "Changed")], None)
        .is_err());
    assert!(h
        .send(vec![h.remove_ix(0, group.index, relative(10))], None)
        .is_err());
    let bob_before = h.record(bob);
    h.send(vec![h.remove_ix(0, alice.index, relative(11))], None)
        .unwrap();
    assert_eq!(h.record(bob), bob_before);
    assert_eq!(h.record(group).child_count, 1);
    assert!(storage::read_record(&h.data(0), alice.index).is_err());
    assert!(h
        .svm
        .get_account(&sa(indexed::index_address(
            &alice.container,
            group,
            &relative(11)
        )
        .0))
        .is_none_or(|a| a.lamports == 0));
    let next = h.add(0, group, relative(11), 2);
    assert!(next.index > bob.index);
    assert!(h
        .svm
        .get_account(&sa(indexed::metadata_address(next).0))
        .is_none());
    assert!(h.send(vec![h.transfer_ix(alice, bob, 0)], None).is_err());
    assert!(h
        .send(vec![h.remove_ix(0, 1, indexed::SOURCE)], None)
        .is_err());
}
#[test]
fn rejects_invalid_inputs_and_rolls_back_across_containers() {
    let mut h = Harness::new(6);
    let root = h.reference(0, 0);
    let alice = h.add(0, root, relative(10), 2);
    let bob = h.add(0, root, relative(11), 2);
    h.transfer(h.reference(0, 1), alice, 100);
    let before = h.snapshot();
    let mut wrong_index = h.add_ix(0, root, relative(12), 2);
    wrong_index.accounts[4].pubkey =
        sa(indexed::index_address(&root.container, root, &relative(13)).0);
    let mut readonly = h.transfer_ix(alice, bob, 1);
    readonly.accounts[1].is_writable = false;
    let mut unsigned = h.transfer_ix(alice, bob, 1);
    unsigned.accounts[0] = AccountMeta::new_readonly(sa(root.container), false);
    for call in [
        h.add_ix(0, root, relative(10), 2),
        h.add_ix(0, alice, relative(12), 2),
        h.add_ix(0, h.reference(0, 99), relative(12), 2),
        h.add_ix(0, root, relative(12), 9),
        h.transfer_ix(h.reference(0, 99), bob, 1),
        h.transfer_ix(alice, root, 1),
        h.transfer_ix(alice, bob, 101),
        wrong_index,
        readonly,
        unsigned,
        h.remove_ix(0, alice.index, relative(10)),
    ] {
        assert!(h.send(vec![call], None).is_err());
        assert_eq!(h.snapshot(), before);
    }
    h.new_container(6, false);
    let carol = h.add(1, root, relative(12), 2);
    let before = h.snapshot();
    assert!(h
        .send(
            vec![
                h.transfer_ix(alice, carol, 10),
                h.transfer_ix(alice, carol, 100)
            ],
            None
        )
        .is_err());
    assert_eq!(h.snapshot(), before);
    // Missing required destination container cannot be replaced with an index.
    let mut missing = h.transfer_ix(alice, carol, 1);
    missing.accounts.pop();
    assert!(h.send(vec![missing], None).is_err());
    assert_eq!(h.snapshot(), before);
    // A second root shares the signer but must not share accounting state.
    let other = h.new_container(6, true);
    let other_root = h.reference(other, 0);
    let stranger = h.add(other, other_root, relative(10), 2);
    assert!(h
        .send(vec![h.transfer_ix(alice, stranger, 1)], None)
        .is_err());
    assert!(h
        .send(vec![h.add_ix(other, root, relative(22), 2)], None)
        .is_err());
}
#[test]
fn overflow_full_container_and_prefunded_index() {
    let mut h = Harness::new(4);
    let root = h.reference(0, 0);
    let source = h.reference(0, 1);
    let address = indexed::index_address(&root.container, root, &relative(10)).0;
    h.svm
        .set_account(
            sa(address),
            Account {
                lamports: 12345,
                ..Account::default()
            },
        )
        .unwrap();
    let alice = h.add(0, root, relative(10), 2);
    let bob = h.add(0, root, relative(11), 2);
    let before = h.snapshot();
    assert!(h
        .send(vec![h.add_ix(0, root, relative(12), 2)], None)
        .is_err());
    assert_eq!(h.snapshot(), before);
    assert!(h
        .svm
        .get_account(&sa(indexed::index_address(
            &root.container,
            root,
            &relative(12)
        )
        .0))
        .is_none());
    h.transfer(source, alice, u128::MAX);
    let before = h.snapshot();
    assert!(h.send(vec![h.transfer_ix(source, bob, 1)], None).is_err());
    assert_eq!(h.snapshot(), before);
    h.transfer(alice, bob, u128::MAX);
    assert_eq!((h.record(alice).debit, h.record(bob).debit), (0, u128::MAX));
}
