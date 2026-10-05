//! Also compiled and run without the mutating program or Anchor SPL movement dependency.
use anchor_lang::{prelude::*, AnchorSerialize};
use cavalre_ledger_core::ledger_lib::Error as CoreError;
use cavalre_ledger_solana::{
    ledger_lib::{root_address, to_address, Record, SOURCE},
    ledger_view::Reader,
    ID,
};

struct Stored {
    key: Pubkey,
    owner: Pubkey,
    lamports: u64,
    data: Vec<u8>,
}
fn stored(key: Pubkey, record: Record) -> Stored {
    let mut data = vec![0; 512];
    data[..8].copy_from_slice(b"CVLEDG01");
    record.serialize(&mut &mut data[8..]).unwrap();
    Stored {
        key,
        owner: ID,
        lamports: 10_000_000,
        data,
    }
}
fn child_slot(parent: Pubkey, index: u32, relative: Option<Pubkey>) -> Stored {
    use cavalre_ledger_solana::ledger_lib::{
        child_index_address, ChildSlot, CHILD_MAGIC, CHILD_SPACE,
    };
    let mut data = vec![0; CHILD_SPACE];
    data[..8].copy_from_slice(CHILD_MAGIC);
    ChildSlot {
        parent,
        index,
        relative,
    }
    .serialize(&mut &mut data[8..])
    .unwrap();
    Stored {
        key: child_index_address(&parent, index).0,
        owner: ID,
        lamports: 10_000_000,
        data,
    }
}
struct Fixture {
    records: Vec<Stored>,
    root: Pubkey,
    app: Pubkey,
    authority: Pubkey,
    relative: Pubkey,
    leaf: Pubkey,
}
impl Fixture {
    fn new() -> Self {
        let mint = Pubkey::new_from_array([20; 32]);
        let authority = Pubkey::new_from_array([21; 32]);
        let relative = Pubkey::new_from_array([22; 32]);
        let (root, bump) = root_address(&Pubkey::default(), &mint);
        let base = Record {
            root,
            parent: cavalre_ledger_solana::ledger_lib::global_root_address().0,
            relative: mint,
            custodian: Pubkey::default(),
            kind: 0,
            token_kind: 2,
            depth: 2,
            registered: true,
            implicit_allowed: true,
            children: 2,
            debit: 50,
            credit: 50,
            name: "Token".into(),
            scope: Pubkey::default(),
            identifier: mint,
            bump,
            sub_index: 1,
        };
        let mut records = vec![stored(root, base.clone())];
        let (source, bump) = to_address(&ID, &root, &SOURCE);
        records.push(stored(
            source,
            Record {
                parent: root,
                relative: SOURCE,
                custodian: source,
                kind: 3,
                token_kind: 0,
                depth: 3,
                children: 0,
                debit: 0,
                name: "Source".into(),
                identifier: Pubkey::default(),
                bump,
                ..base.clone()
            },
        ));
        let (app, bump) = to_address(&ID, &root, &authority);
        records.push(stored(
            app,
            Record {
                parent: root,
                relative: authority,
                custodian: app,
                token_kind: 0,
                depth: 3,
                children: 0,
                implicit_allowed: false,
                credit: 0,
                name: "Application".into(),
                sub_index: 2,
                identifier: Pubkey::default(),
                bump,
                ..base.clone()
            },
        ));
        let (leaf, bump) = to_address(&ID, &app, &relative);
        records.push(stored(
            leaf,
            Record {
                parent: app,
                relative,
                custodian: app,
                kind: 2,
                token_kind: 0,
                depth: 4,
                registered: false,
                sub_index: 0,
                children: 0,
                credit: 0,
                name: String::new(),
                identifier: Pubkey::default(),
                bump,
                ..base
            },
        ));
        records.push(child_slot(root, 0, Some(SOURCE)));
        records.push(child_slot(root, 1, Some(authority)));
        Self {
            records,
            root,
            app,
            authority,
            relative,
            leaf,
        }
    }
    fn reader(&self) -> Reader {
        let mut reader = Reader::new();
        for r in &self.records {
            reader.insert(r.key, &r.owner, &r.data).unwrap();
        }
        reader
    }
}

#[test]
fn nonsigner_readonly_accounts_work_without_executing_a_mutation_program() {
    let mut fixture = Fixture::new();
    let before: Vec<_> = fixture
        .records
        .iter()
        .map(|r| (r.lamports, r.data.clone()))
        .collect();
    let accounts: Vec<_> = fixture
        .records
        .iter_mut()
        .map(|r| {
            AccountInfo::new(
                &r.key,
                false,
                false,
                &mut r.lamports,
                &mut r.data,
                &r.owner,
                false,
            )
        })
        .collect();
    let reader = Reader::from_account_infos(&accounts).unwrap();
    assert!(accounts.iter().all(|a| !a.is_signer && !a.is_writable));
    let view = reader
        .account_view(&fixture.root, &fixture.app, &fixture.relative)
        .unwrap();
    assert_eq!(view.custodian, fixture.authority);
    assert!(!view.registered && !view.admitted);
    assert_eq!(
        reader
            .balance_of(&fixture.root, &fixture.app, &fixture.relative)
            .unwrap(),
        50
    );
    assert_eq!(reader.total_supply(&fixture.root).unwrap(), 50);
    drop(accounts);
    assert_eq!(
        fixture
            .records
            .iter()
            .map(|r| (r.lamports, r.data.clone()))
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn rpc_reader_distinguishes_unknown_absent_and_allocated_implicit_accounts() {
    let fixture = Fixture::new();
    let mut reader = fixture.reader();
    let relative = Pubkey::new_from_array([23; 32]);
    assert_eq!(
        reader.account_view(&fixture.root, &fixture.app, &relative),
        Err(CoreError::MissingAccount)
    );
    reader
        .insert_missing(to_address(&ID, &fixture.app, &relative).0)
        .unwrap();
    assert_eq!(
        reader
            .balance_of(&fixture.root, &fixture.app, &relative)
            .unwrap(),
        0
    );
    assert_eq!(
        reader
            .balance_of(&fixture.root, &fixture.app, &fixture.relative)
            .unwrap(),
        50
    );
    assert_eq!(reader.name(&fixture.leaf).unwrap(), "");
    assert_eq!(reader.known_ledgers().unwrap(), vec![fixture.root]);
}

#[test]
fn registered_child_listing_uses_slots_without_child_records() {
    let fixture = Fixture::new();
    let reader = fixture.reader();
    assert!(reader
        .sub_accounts(&fixture.root, &fixture.app, 0, 10)
        .unwrap()
        .is_empty());
    let expected = vec![SOURCE, fixture.authority];

    assert_eq!(
        reader
            .sub_accounts(&fixture.root, &fixture.root, 0, 10)
            .unwrap(),
        expected
    );
    let mut partial = Reader::new();
    for r in fixture.records.iter().filter(|r| r.key != fixture.app) {
        partial.insert(r.key, &r.owner, &r.data).unwrap();
    }
    assert_eq!(
        partial.sub_accounts(&fixture.root, &fixture.root, 0, 10),
        Ok(expected)
    );
}

#[test]
fn readers_reject_wrong_owners_addresses_headers_duplicates_and_root_context() {
    let fixture = Fixture::new();
    let record = &fixture.records[0];
    let mut reader = Reader::new();
    assert!(reader
        .insert(record.key, &Pubkey::default(), &record.data)
        .is_err());
    assert!(reader
        .insert(Pubkey::new_from_array([25; 32]), &ID, &record.data)
        .is_err());
    let mut corrupt = record.data.clone();
    corrupt[0] ^= 1;
    assert!(reader.insert(record.key, &ID, &corrupt).is_err());
    reader.insert(record.key, &ID, &record.data).unwrap();
    assert!(reader.insert(record.key, &ID, &record.data).is_err());
    assert!(reader.insert_missing(record.key).is_err());
    let reader = fixture.reader();
    assert_eq!(
        reader.total_supply(&fixture.app),
        Err(CoreError::InvalidAccount)
    );
    let mut partial = Reader::new();
    let leaf = fixture.records.last().unwrap();
    partial.insert(leaf.key, &ID, &leaf.data).unwrap();
    assert_eq!(partial.name(&fixture.leaf), Err(CoreError::MissingAccount));
}

#[test]
fn partial_pages_authenticate_slots_and_need_no_sibling_records() {
    use cavalre_ledger_solana::ledger_lib::child_index_address;
    let fixture = Fixture::new();
    let root = &fixture.records[0];
    let slot = child_slot(fixture.root, 1, Some(fixture.authority));
    let mut reader = Reader::new();
    reader.insert(root.key, &root.owner, &root.data).unwrap();
    reader.insert(slot.key, &slot.owner, &slot.data).unwrap();
    assert_eq!(
        reader.sub_accounts(&fixture.root, &fixture.root, 1, 1),
        Ok(vec![fixture.authority])
    );
    assert_eq!(
        reader.sub_account(&fixture.root, &fixture.root, 1),
        Ok(fixture.authority)
    );
    assert_eq!(
        reader.sub_accounts(&fixture.root, &fixture.root, 0, 2),
        Err(CoreError::IncompleteIndex)
    );
    assert_eq!(
        reader.sub_accounts(&fixture.root, &fixture.root, usize::MAX, usize::MAX),
        Ok(vec![])
    );
    assert_eq!(
        reader.sub_accounts(&fixture.root, &fixture.root, 0, 0),
        Ok(vec![])
    );
    assert!(Reader::new()
        .insert(slot.key, &Pubkey::default(), &slot.data)
        .is_err());
    assert!(Reader::new()
        .insert(child_index_address(&fixture.root, 0).0, &ID, &slot.data)
        .is_err());
    assert!(Reader::new()
        .insert(child_index_address(&fixture.app, 1).0, &ID, &slot.data)
        .is_err());
    assert!(Reader::new()
        .insert(slot.key, &ID, &slot.data[..slot.data.len() - 1])
        .is_err());
    assert!(reader.insert(slot.key, &ID, &slot.data).is_err());
}
