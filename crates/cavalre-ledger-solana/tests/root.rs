//! Root uses ordinary records and child queries, including without mutations.
use anchor_lang::{prelude::Pubkey, AnchorSerialize};
use cavalre_ledger_core::ledger_lib::Error;
use cavalre_ledger_solana::{
    ledger_lib::{decode_data, global_root_address, root_address, Record, ROOT_NAME},
    ledger_view::Reader,
    ID,
};
fn bytes(record: &Record) -> Vec<u8> {
    let mut data = vec![0; 512];
    data[..8].copy_from_slice(b"CVLEDG01");
    record.serialize(&mut &mut data[8..]).unwrap();
    data
}
fn root(children: u32) -> Record {
    let (key, bump) = global_root_address();
    Record {
        root: key,
        parent: key,
        relative: key,
        custodian: key,
        kind: 0,
        token_kind: 0,
        depth: 1,
        registered: true,
        implicit_allowed: false,
        children,
        debit: 0,
        credit: 0,
        name: ROOT_NAME.into(),
        scope: Pubkey::default(),
        identifier: Pubkey::default(),
        bump,
    }
}
fn ledger(tag: u8) -> (Pubkey, Record) {
    let identifier = Pubkey::new_from_array([tag; 32]);
    let (address, bump) = root_address(&Pubkey::default(), &identifier);
    let mut record = root(1);
    record.root = address;
    record.relative = identifier;
    record.identifier = identifier;
    record.depth = 2;
    record.token_kind = 2;
    record.implicit_allowed = true;
    record.name = "Token".into();
    record.bump = bump;
    (address, record)
}
#[test]
fn discovery_is_exactly_root_child_enumeration_and_requires_a_complete_snapshot() {
    let mut reader = Reader::new();
    let global = global_root_address().0;
    assert_eq!(reader.ledger_count(), Err(Error::MissingAccount));
    reader.insert(global, &ID, &bytes(&root(2))).unwrap();
    assert_eq!(reader.name(&global).unwrap(), "Root");
    assert_eq!(reader.ledger_count(), Ok(2));
    assert_eq!(
        reader.ledger_count(),
        reader.sub_account_count(&global, &global)
    );
    let (first, record) = ledger(90);
    reader.insert(first, &ID, &bytes(&record)).unwrap();
    assert_eq!(reader.ledgers(0, 2), Err(Error::IncompleteIndex));
    assert_eq!(reader.ledger_at(0), Err(Error::IncompleteIndex));
    let (second, record) = ledger(10);
    reader.insert(second, &ID, &bytes(&record)).unwrap();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(reader.ledgers(0, usize::MAX), Ok(expected.clone()));
    assert_eq!(
        reader.ledgers(0, usize::MAX),
        reader.sub_accounts(&global, &global, 0, usize::MAX)
    );
    assert_eq!(reader.ledger_at(0), reader.sub_account(&global, &global, 0));
    assert_eq!(reader.ledgers(1, 1), Ok(vec![expected[1]]));
    assert_eq!(reader.ledger_at(2), Err(Error::InvalidIndex));
    assert_eq!(reader.ledgers(usize::MAX, usize::MAX), Ok(vec![]));
    assert_eq!(reader.ledgers(0, 0), Ok(vec![]));
    // Root is not a token ledger or monetary endpoint.
    assert_eq!(reader.total_supply(&global), Err(Error::InvalidAccount));
    assert_eq!(reader.ledger(&global), Ok(None));
}
#[test]
fn root_uses_the_shared_record_decoder_and_rejects_forged_state_and_duplicates() {
    let global = global_root_address().0;
    let other = Pubkey::new_from_array([12; 32]);
    let record = root(1);
    let data = bytes(&record);
    assert!(decode_data(&global, &other, &data).is_err());
    assert!(decode_data(&other, &ID, &data).is_err());
    assert!(decode_data(&global, &ID, &data[..511]).is_err());
    for corrupt in [
        Record {
            parent: other,
            ..record.clone()
        },
        Record {
            root: other,
            ..record.clone()
        },
        Record {
            debit: 1,
            ..record.clone()
        },
        Record {
            kind: 1,
            ..record.clone()
        },
        Record {
            registered: false,
            ..record.clone()
        },
    ] {
        assert!(decode_data(&global, &ID, &bytes(&corrupt)).is_err());
    }
    let mut reader = Reader::new();
    reader.insert(global, &ID, &data).unwrap();
    assert!(reader.insert(global, &ID, &data).is_err());
    assert!(reader.insert_missing(global).is_err());
    let (address, mut ledger) = ledger(12);
    ledger.parent = other;
    assert!(reader.insert(address, &ID, &bytes(&ledger)).is_err());
}
