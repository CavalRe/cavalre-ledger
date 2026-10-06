//! Root children exercised through the actual Ledger sBPF program.
use super::*;
use ledger::ledger_lib::global_root_address;
use ledger::ledger_view::Reader;
fn count(h: &Harness) -> u32 {
    h.record(sa(global_root_address().0)).children
}
fn add_internal(h: &Harness, id: Address, source: bool) -> Instruction {
    let root = sa(ledger::ledger_lib::root_storage_address(&ap(h.key(0)), &ap(id)).0);
    ix(
        h.registration(0, root),
        instruction::AddLedger {
            id: ap(id),
            name: "Units".into(),
            symbol: "UNIT".into(),
            decimals: 6,
        },
        &if source {
            vec![child(root, sa(SOURCE))]
        } else {
            vec![]
        },
    )
}
#[test]
fn all_ledger_kinds_are_discovered_as_root_children() {
    let mut h = Harness::new();
    let (internal, _) = h.internal();
    let classic = External::new(&mut h, 60, 0);
    let token2022 = External::setup(&mut h, 61, 0, TOKEN_2022);
    let registration = token2022.registration(&h);
    succeeds(&mut h, &[0], registration);
    let native = h.external_root(sa(ledger::ledger_lib::NATIVE_SOL));
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", h.storage(native).as_ref()],
        &ledger::ID,
    )
    .0);
    let i = ix(
        accounts::RegisterSol {
            payer: ap(h.key(0)),
            root: ap(h.storage(native)),
            vault: ap(vault),
            system_program: ap(SYSTEM),
            global_root: global_root_address().0,
        },
        instruction::AddNativeSol {},
        &[child(native, sa(SOURCE))],
    );
    succeeds(&mut h, &[0], i);
    let expected = vec![internal, classic.root, token2022.root, native];
    let global = sa(global_root_address().0);
    assert_eq!(count(&h), 4);
    let before = h.account(&global).unwrap();
    let mut reader = Reader::new();
    reader
        .insert(ap(global), &ap(before.owner), &before.data)
        .unwrap();
    for (index, address) in expected.iter().enumerate() {
        assert_eq!(
            ledger::ledger_storage::child(&before.data, index as u32).unwrap(),
            Some(ap(*address))
        );
        let r = h.record(*address);
        assert_eq!(r.parent, ap(global));
        assert_eq!(r.custodian, ap(global));
        assert_eq!(r.depth, 2);
        let a = h.account(&h.storage(*address)).unwrap();
        reader
            .insert(ap(h.storage(*address)), &ap(a.owner), &a.data)
            .unwrap();
    }
    assert_eq!(
        reader.ledgers(0, usize::MAX).unwrap(),
        expected.into_iter().map(ap).collect::<Vec<_>>()
    );
    assert_eq!(
        reader.ledger_count(),
        reader.sub_account_count(&ap(global), &ap(global))
    );
    assert_eq!(
        reader.ledgers(0, usize::MAX),
        reader.sub_accounts(&ap(global), &ap(global), 0, usize::MAX)
    );
    assert_eq!(
        reader.ledger_at(0),
        reader.sub_account(&ap(global), &ap(global), 0)
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
    assert_eq!(h.account(&global).unwrap(), before);
    assert_eq!(h.record(internal).debit, 20);
    assert_eq!(h.record(internal).credit, 20);
}
#[test]
fn root_child_creation_needs_no_source_or_append_slot_account() {
    let mut h = Harness::new();
    let id = Address::new_from_array([97; 32]);
    let first = add_internal(&h, id, false);
    let next = h.indexed(add_internal(&h, Address::new_from_array([98; 32]), false));
    succeeds(&mut h, &[0], first);
    assert_eq!(count(&h), 1);
    // The maintained vector determines the current append position atomically.
    assert!(run_raw(&mut h, &[0], next).is_ok());
    assert_eq!(count(&h), 2);
    let mut conflict = add_internal(&h, id, false);
    conflict.data = instruction::AddLedger {
        id: ap(id),
        name: "Conflicting".into(),
        symbol: "INT".into(),
        decimals: 0,
    }
    .data();
    rejects(&mut h, &[0], conflict, LedgerError::MetadataConflict.into());
    assert_eq!(count(&h), 2);
}
#[test]
fn creation_rejects_fake_root_readonly_root_and_child_count_overflow() {
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
    readonly.accounts[4].is_writable = false;
    rejects(&mut h, &[0], readonly, u32::from(ErrorCode::ConstraintMut));
    h.internal();
    // Corrupt a fixture to exercise the checked numeric boundary, not a policy cap.
    let key = sa(global_root_address().0);
    let mut a = h.account(&key).unwrap();
    let mut record = h.record(key);
    record.children = u32::MAX;
    anchor_lang::AnchorSerialize::serialize(&record, &mut &mut a.data[8..]).unwrap();
    h.svm.set_account(key, a).unwrap();
    let i = add_internal(&h, Address::new_from_array([99; 32]), true);
    rejects(&mut h, &[0], i, LedgerError::InvalidAccount as u32 + 6000);
}
