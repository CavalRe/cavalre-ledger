//! Global Root and registry exercised through the actual Ledger sBPF program.
use super::*;
use ledger::ledger_lib::{global_root_address, ledger_index_address, GlobalRoot};
use ledger::ledger_view::Reader;
fn count(h: &Harness) -> u64 {
    let key = global_root_address().0;
    let a = h.svm.get_account(&sa(key)).unwrap();
    GlobalRoot::decode(&key, &ap(a.owner), &a.data)
        .unwrap()
        .ledger_count
}
fn add_internal(h: &Harness, id: Address, source: bool) -> Instruction {
    let root = sa(ledger::ledger_lib::root_address(&ap(h.key(0)), &ap(id)).0);
    ix(
        h.registration(0, root),
        instruction::AddLedger {
            id: ap(id),
            name: "Units".into(),
        },
        &if source {
            vec![child(root, sa(SOURCE))]
        } else {
            vec![]
        },
    )
}
#[test]
fn all_ledger_kinds_share_root_registry_in_creation_order() {
    let mut h = Harness::new();
    let (internal, _) = h.internal();
    let classic = External::new(&mut h, 60, 0);
    let token2022 = External::setup(&mut h, 61, 0, TOKEN_2022);
    let registration = token2022.registration(&h, "Token2022");
    succeeds(&mut h, &[0], registration);
    let native =
        sa(ledger::ledger_lib::root_address(&ap(SYSTEM), &ledger::ledger_lib::NATIVE_SOL).0);
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", native.as_ref()],
        &ledger::ID,
    )
    .0);
    let i = ix(
        accounts::RegisterSol {
            payer: ap(h.key(0)),
            root: ap(native),
            vault: ap(vault),
            system_program: ap(SYSTEM),
            global_root: global_root_address().0,
            ledger_index: h.next_index(),
        },
        instruction::AddNativeSol {},
        &[child(native, sa(SOURCE))],
    );
    succeeds(&mut h, &[0], i);
    let expected = vec![internal, classic.root, token2022.root, native];
    let global = sa(global_root_address().0);
    assert_eq!(count(&h), 4);
    let before = h.svm.get_account(&global).unwrap();
    let mut reader = Reader::new();
    reader
        .insert(ap(global), &ap(before.owner), &before.data)
        .unwrap();
    for (index, address) in expected.iter().enumerate() {
        let r = h.record(*address);
        assert_eq!(r.parent, ap(global));
        assert_eq!(r.custodian, ap(global));
        assert_eq!(r.depth, 2);
        let key = ledger_index_address(index as u64).0;
        let a = h.svm.get_account(&sa(key)).unwrap();
        reader.insert(key, &ap(a.owner), &a.data).unwrap();
    }
    assert_eq!(
        reader.ledgers(0, u64::MAX).unwrap(),
        expected.into_iter().map(ap).collect::<Vec<_>>()
    );
    // Posting stays within one asset. Global Root isn't an instruction account.
    let to = child(internal, h.key(1));
    let i = ix(
        h.base(0, internal),
        instruction::Transfer {
            from_parent: ap(internal),
            from: SOURCE,
            to_parent: ap(internal),
            to: ap(h.key(1)),
            amount: 20,
        },
        &[child(internal, sa(SOURCE)), to],
    );
    assert!(!i.accounts.iter().any(|a| a.pubkey == global));
    succeeds(&mut h, &[0], i);
    assert_eq!(h.svm.get_account(&global).unwrap(), before);
    assert_eq!(h.record(internal).debit, 20);
    assert_eq!(h.record(internal).credit, 20);
}
#[test]
fn failed_initialization_and_stale_registry_slots_roll_back_every_account() {
    let mut h = Harness::new();
    let id = h.key(2);
    let i = add_internal(&h, id, false); // Source missing: registry writes precede commit.
    rejects(&mut h, &[0], i, LedgerError::MissingAccount as u32 + 6000);
    assert!(h.svm.get_account(&sa(global_root_address().0)).is_none());
    assert!(h.svm.get_account(&sa(ledger_index_address(0).0)).is_none());
    let stale = add_internal(&h, Address::new_from_array([98; 32]), true);
    h.internal();
    rejects(
        &mut h,
        &[0],
        stale,
        LedgerError::InvalidAccount as u32 + 6000,
    );
    assert_eq!(count(&h), 1);
    let duplicate = add_internal(&h, id, true);
    rejects(
        &mut h,
        &[0],
        duplicate,
        LedgerError::InvalidAccount as u32 + 6000,
    );
    let fresh = add_internal(&h, Address::new_from_array([98; 32]), true);
    succeeds(&mut h, &[0], fresh);
    assert_eq!(count(&h), 2);
    let failed = add_internal(&h, Address::new_from_array([99; 32]), false);
    rejects(
        &mut h,
        &[0],
        failed,
        LedgerError::MissingAccount as u32 + 6000,
    );
    assert_eq!(count(&h), 2);
    assert!(h.svm.get_account(&sa(ledger_index_address(2).0)).is_none());
}
#[test]
fn registration_rejects_fake_root_readonly_index_and_count_overflow() {
    let mut h = Harness::new();
    let id = h.key(2);
    let mut fake = add_internal(&h, id, true);
    fake.accounts[4].pubkey = Address::new_from_array([97; 32]);
    rejects(
        &mut h,
        &[0],
        fake,
        LedgerError::InvalidAccount as u32 + 6000,
    );
    let mut readonly = add_internal(&h, id, true);
    readonly.accounts[5].is_writable = false;
    rejects(&mut h, &[0], readonly, u32::from(ErrorCode::ConstraintMut));
    h.internal();
    // Corrupt a fixture to exercise the checked numeric boundary, not a policy cap.
    let key = sa(global_root_address().0);
    let mut a = h.svm.get_account(&key).unwrap();
    a.data[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
    h.svm.set_account(key, a).unwrap();
    let i = add_internal(&h, Address::new_from_array([99; 32]), true);
    rejects(&mut h, &[0], i, LedgerError::Accounting as u32 + 6000);
}
