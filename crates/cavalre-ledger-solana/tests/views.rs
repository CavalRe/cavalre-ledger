//! Read-only coverage of the packed layout, independent metadata and typed PDAs.
use anchor_lang::prelude::*;
use cavalre_ledger_core::ledger_lib::Error;
use cavalre_ledger_solana::{ledger_lib::*, ledger_storage as storage, ledger_view::Reader, ID};

fn key(n: u8) -> Pubkey {
    Pubkey::new_from_array([n; 32])
}
fn fixture() -> (Reader, Pubkey, Pubkey, Pubkey, Record) {
    let ledger = ledger_pda(&key(1), &key(2)).0;
    let app = to_address(&ID, &ledger, &key(3)).0;
    let leaf = to_address(&ID, &app, &key(4)).0;
    let root = Record {
        parent: GLOBAL_ROOT,
        custodian: GLOBAL_ROOT,
        kind: 0,
        depth: 2,
        debit: 50,
        credit: 50,
        child_index: 1,
        ledger: Some(LedgerConfig {
            token_kind: 3,
            authority: key(1),
            identifier: key(2),
            vault: Pubkey::default(),
        }),
        children: vec![key(3)],
    };
    let group = Record {
        parent: ledger,
        custodian: app,
        kind: 0,
        depth: 3,
        debit: 50,
        credit: 0,
        child_index: 1,
        ledger: None,
        children: vec![key(4)],
    };
    let record = Record {
        parent: app,
        custodian: app,
        kind: 2,
        depth: 4,
        debit: 50,
        credit: 0,
        child_index: 1,
        ledger: None,
        children: vec![],
    };
    let mut reader = Reader::new();
    for (address, r) in [(ledger, root), (app, group), (leaf, record.clone())] {
        reader.insert(address, &ID, &r.to_bytes().unwrap()).unwrap();
    }
    let relative = ledger_relative(&key(1), &key(2));
    reader
        .insert_metadata(
            metadata_address(&GLOBAL_ROOT, &relative).0,
            &ID,
            &storage::encode_metadata(&Metadata {
                bump: ledger_pda(&key(1), &key(2)).1,
                decimals: 18,
                name: "Units".into(),
                symbol: "UNIT".into(),
            })
            .unwrap(),
            &GLOBAL_ROOT,
            &relative,
        )
        .unwrap();
    (reader, ledger, app, leaf, record)
}
#[test]
fn direct_balances_children_and_metadata_are_independent() {
    let (reader, ledger, app, leaf, record) = fixture();
    assert_eq!(record.space(), 106);
    assert_eq!(reader.balance_of(&ledger, &app, &key(4)), Ok(50));
    assert_eq!(reader.ledger(&leaf), Ok(Some(ledger)));
    assert_eq!(
        reader.sub_accounts(&ledger, &app, 0, usize::MAX),
        Ok(vec![key(4)])
    );
    assert_eq!(reader.sub_account_index(&leaf), Ok(1));
    assert_eq!(reader.name(&leaf), Ok(String::new()));
    assert_eq!(reader.symbol(&leaf), Ok(Some("UNIT".into())));
    assert_eq!(reader.decimals(&leaf), Ok(Some(18)));
    assert!(
        reader
            .account_view(&ledger, &app, &key(4))
            .unwrap()
            .registered
    );
}
#[test]
fn unknown_and_absent_accounts_remain_distinct() {
    let (mut reader, ledger, app, _, _) = fixture();
    let missing = to_address(&ID, &app, &key(5)).0;
    assert_eq!(
        reader.balance_of(&ledger, &app, &key(5)),
        Err(Error::MissingAccount)
    );
    reader.insert_missing(missing).unwrap();
    assert_eq!(reader.balance_of(&ledger, &app, &key(5)), Ok(0));
    assert_eq!(
        reader.sub_accounts(&ledger, &app, usize::MAX, 1),
        Ok(vec![])
    );
}
#[test]
fn fixed_offsets_and_vector_lengths_reject_malformed_data() {
    let (_, _, _, _, record) = fixture();
    let mut bytes = record.to_bytes().unwrap();
    for n in 0..bytes.len() {
        assert!(storage::decode(&bytes[..n]).is_err());
    }
    assert_eq!(storage::balance(&bytes, storage::DEBIT), 50);
    storage::set_balance(&mut bytes, storage::DEBIT, u128::MAX);
    assert_eq!(storage::decode(&bytes).unwrap().debit, u128::MAX);
    bytes[storage::HEADER_LEN..storage::HEADER_LEN + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(storage::decode(&bytes).is_err());
    let mut bytes = record.to_bytes().unwrap();
    bytes.push(0);
    assert!(storage::decode(&bytes).is_err());
}
#[test]
fn wrong_owner_identity_and_metadata_namespace_are_rejected() {
    let (mut reader, ledger, app, leaf, record) = fixture();
    assert!(reader
        .insert(key(8), &key(9), &record.to_bytes().unwrap())
        .is_err());
    assert!(reader
        .insert(leaf, &ID, &record.to_bytes().unwrap())
        .is_err());
    let fake = key(8);
    reader
        .insert(fake, &ID, &record.to_bytes().unwrap())
        .unwrap();
    assert_eq!(reader.total_supply(&fake), Err(Error::InvalidAccount));
    let meta = Metadata {
        bump: to_address(&ID, &app, &key(4)).1,
        decimals: 0,
        name: "Leaf".into(),
        symbol: String::new(),
    };
    let bytes = storage::encode_metadata(&meta).unwrap();
    assert!(reader
        .insert_metadata(leaf, &ID, &bytes, &app, &key(4))
        .is_err());
    assert!(reader
        .insert_metadata(
            metadata_address(&app, &key(4)).0,
            &key(9),
            &bytes,
            &app,
            &key(4)
        )
        .is_err());
    assert_eq!(reader.total_supply(&ledger), Ok(50));
}
#[test]
fn readonly_account_infos_preserve_bytes_and_balances() {
    let (_, _, app, leaf, record) = fixture();
    let parent_record = Record {
        parent: key(9),
        custodian: app,
        kind: 0,
        depth: 3,
        debit: 50,
        credit: 0,
        child_index: 1,
        ledger: None,
        children: vec![key(4)],
    };
    let mut data = record.to_bytes().unwrap();
    let before = data.clone();
    let mut lamports = 10;
    let account = AccountInfo::new(&leaf, false, false, &mut lamports, &mut data, &ID, false);
    assert_eq!(
        cavalre_ledger_solana::ledger_view::debit_balance_of(&account).unwrap(),
        50
    );
    assert!(!account.is_writable && !account.is_signer);
    drop(account);
    assert_eq!(data, before);
    assert_eq!(lamports, 10);
    assert_eq!(parent_record.children.len(), 1);
}

#[test]
fn runtime_reader_uses_namespace_when_metadata_looks_like_an_account() {
    let (_, ledger, app, leaf, record) = fixture();
    let parent = Record {
        parent: ledger,
        custodian: app,
        kind: 0,
        depth: 3,
        debit: 50,
        credit: 0,
        child_index: 1,
        ledger: None,
        children: vec![key(4)],
    };
    let mut name = vec![0; 64];
    name[58] = 2; // Accounting kind offset 64.
    name[59] = 3; // Accounting depth offset 65.
    let mut symbol = vec![0; 32];
    symbol[24] = 1; // Accounting child_index offset 98.
    let metadata = Metadata {
        bump: to_address(&ID, &app, &key(4)).1,
        decimals: 0,
        name: String::from_utf8(name).unwrap(),
        symbol: String::from_utf8(symbol).unwrap(),
    };
    let metadata_key = metadata_address(&app, &key(4)).0;
    let mut metadata_bytes = storage::encode_metadata(&metadata).unwrap();
    assert!(storage::decode(&metadata_bytes).is_ok());
    let mut parent_bytes = parent.to_bytes().unwrap();
    let mut leaf_bytes = record.to_bytes().unwrap();
    let (mut parent_lamports, mut leaf_lamports, mut metadata_lamports) = (1, 1, 1);
    let accounts = [
        AccountInfo::new(
            &metadata_key,
            false,
            false,
            &mut metadata_lamports,
            &mut metadata_bytes,
            &ID,
            false,
        ),
        AccountInfo::new(
            &app,
            false,
            false,
            &mut parent_lamports,
            &mut parent_bytes,
            &ID,
            false,
        ),
        AccountInfo::new(
            &leaf,
            false,
            false,
            &mut leaf_lamports,
            &mut leaf_bytes,
            &ID,
            false,
        ),
    ];
    let reader = Reader::from_account_infos(&accounts).unwrap();
    assert_eq!(reader.name(&leaf), Ok(metadata.name));
}
