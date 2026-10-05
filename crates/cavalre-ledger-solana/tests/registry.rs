//! Registry reads remain available when the mutation module is excluded.
use anchor_lang::{prelude::Pubkey, AnchorSerialize};
use cavalre_ledger_core::ledger_lib::Error;
use cavalre_ledger_solana::{
    ledger_lib::{global_root_address, ledger_index_address, GlobalRoot, LedgerEntry},
    ledger_view::Reader,
    ID,
};
fn bytes(magic: &[u8; 8], value: &impl AnchorSerialize) -> Vec<u8> {
    let mut data = magic.to_vec();
    value.serialize(&mut data).unwrap();
    data
}
fn root(count: u64) -> Vec<u8> {
    bytes(
        b"CVROOT01",
        &GlobalRoot {
            ledger_count: count,
        },
    )
}
fn entry(index: u64, ledger: Pubkey) -> Vec<u8> {
    bytes(b"CVIDX001", &LedgerEntry { index, ledger })
}
#[test]
fn count_and_pages_use_authenticated_root_and_only_requested_slots() {
    let mut reader = Reader::new();
    assert_eq!(reader.ledger_count(), Err(Error::MissingAccount));
    reader
        .insert(global_root_address().0, &ID, &root(3))
        .unwrap();
    assert_eq!(reader.name(&global_root_address().0).unwrap(), "Root");
    assert_eq!(reader.ledger_count(), Ok(3));
    let first = Pubkey::new_from_array([90; 32]);
    let second = Pubkey::new_from_array([10; 32]);
    reader
        .insert(ledger_index_address(1).0, &ID, &entry(1, first))
        .unwrap();
    reader
        .insert(ledger_index_address(2).0, &ID, &entry(2, second))
        .unwrap();
    // Registration order, not address order. No token ledger records supplied.
    assert_eq!(reader.ledgers(1, u64::MAX), Ok(vec![first, second]));
    assert_eq!(reader.ledger_at(1), Ok(first));
    assert_eq!(reader.ledger_at(0), Err(Error::IncompleteIndex));
    assert_eq!(reader.ledgers(0, 3), Err(Error::IncompleteIndex));
    assert_eq!(reader.ledger_at(3), Err(Error::InvalidIndex));
    assert_eq!(reader.ledgers(u64::MAX, u64::MAX), Ok(vec![]));
    assert_eq!(reader.ledgers(0, 0), Ok(vec![]));
}
#[test]
fn registry_rejects_forged_owners_addresses_layouts_and_duplicate_inputs() {
    let global = global_root_address().0;
    let slot = ledger_index_address(0).0;
    let other = Pubkey::new_from_array([12; 32]);
    assert!(GlobalRoot::decode(&global, &other, &root(1)).is_err());
    assert!(GlobalRoot::decode(&other, &ID, &root(1)).is_err());
    assert!(GlobalRoot::decode(&global, &ID, &root(1)[..15]).is_err());
    assert!(LedgerEntry::decode(&slot, &other, &entry(0, other)).is_err());
    assert!(LedgerEntry::decode(&slot, &ID, &entry(1, other)).is_err());
    assert!(LedgerEntry::decode(&slot, &ID, &root(1)).is_err());
    let mut reader = Reader::new();
    reader.insert(global, &ID, &root(1)).unwrap();
    assert!(reader.insert(global, &ID, &root(1)).is_err());
    reader.insert(slot, &ID, &entry(0, other)).unwrap();
    assert!(reader.insert(slot, &ID, &entry(0, other)).is_err());
    assert!(reader.insert_missing(slot).is_err());
}
