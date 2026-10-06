use anchor_lang::prelude::Pubkey;
use cavalre_ledger_solana::{ledger_lib::*, ledger_view::Reader, ID};
#[test]
fn global_root_enumerates_inline_relative_identities_as_ledger_pdas() {
    let ids = [Pubkey::new_unique(), Pubkey::new_unique()];
    let record = Record {
        parent: GLOBAL_ROOT,
        custodian: GLOBAL_ROOT,
        kind: 0,
        depth: 1,
        debit: 0,
        credit: 0,
        child_index: 0,
        ledger: None,
        children: ids.to_vec(),
    };
    let mut reader = Reader::new();
    reader
        .insert(GLOBAL_ROOT, &ID, &record.to_bytes().unwrap())
        .unwrap();
    assert_eq!(reader.ledger_count(), Ok(2));
    assert_eq!(
        reader.ledgers(0, usize::MAX),
        Ok(ids.map(|id| to_address(&ID, &GLOBAL_ROOT, &id).0).to_vec())
    );
    assert_eq!(reader.name(&GLOBAL_ROOT), Ok("Root".into()));
    assert_eq!(reader.ledger(&GLOBAL_ROOT), Ok(None));
    assert!(reader
        .insert(GLOBAL_ROOT, &ID, &record.to_bytes().unwrap())
        .is_err());
    assert!(decode_data(&Pubkey::new_unique(), &ID, &record.to_bytes().unwrap()).is_err());
    let mut bytes = record.to_bytes().unwrap();
    bytes[0] ^= 1;
    assert!(decode_data(&GLOBAL_ROOT, &ID, &bytes).is_err());
}
