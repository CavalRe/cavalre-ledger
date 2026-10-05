//! Maintained child slots and reverse indexes, matching Solidity subs/subIndex.
use super::*;
use ledger::ledger_lib::{child_index_address, decode_child_data};
use ledger::ledger_view::Reader;

fn page(h: &Harness, root: Address, start: u32, limit: u32) -> Vec<Address> {
    let mut reader = Reader::new();
    let record = h.svm.get_account(&root).unwrap();
    reader
        .insert(ap(root), &ap(record.owner), &record.data)
        .unwrap();
    let count = h.record(root).children;
    for index in start..start.saturating_add(limit).min(count) {
        let key = sa(child_index_address(&ap(root), index).0);
        let slot = h.svm.get_account(&key).unwrap();
        reader.insert(ap(key), &ap(slot.owner), &slot.data).unwrap();
    }
    reader
        .sub_accounts(&ap(root), &ap(root), start as usize, limit as usize)
        .unwrap()
        .into_iter()
        .map(sa)
        .collect()
}

#[test]
fn child_slots_preserve_insertion_order_swap_pop_and_reuse_without_sibling_reads() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let values = [50, 20, 90, 10].map(|n| Address::new_from_array([n; 32]));
    for relative in values {
        let i = leaf(&h, root, h.key(0), root, relative, "Leaf", false);
        succeeds(&mut h, &[0], i);
    }
    assert_eq!(
        page(&h, root, 0, 10),
        [vec![sa(SOURCE)], values.to_vec()].concat()
    );
    assert_eq!(page(&h, root, 2, 2), values[1..3]);
    let i = remove(&h, root, h.key(0), root, values[1], false);
    succeeds(&mut h, &[0], i);
    assert_eq!(
        page(&h, root, 0, 10),
        vec![sa(SOURCE), values[0], values[3], values[2]]
    );
    assert_eq!(h.record(child(root, values[3])).sub_index, 3);
    assert_eq!(h.record(child(root, values[1])).sub_index, 0);
    let last_key = sa(child_index_address(&ap(root), 4).0);
    let last = h.svm.get_account(&last_key).unwrap();
    assert_eq!(
        decode_child_data(&ap(last_key), &ap(last.owner), &last.data)
            .unwrap()
            .relative,
        None
    );
    let i = leaf(&h, root, h.key(0), root, values[1], "Leaf", false);
    succeeds(&mut h, &[0], i);
    assert_eq!(
        h.svm.get_account(&last_key).unwrap().lamports,
        last.lamports
    );
    assert_eq!(page(&h, root, 4, 1), vec![values[1]]);
    assert_eq!(h.record(child(root, values[1])).sub_index, 5);
    let i = remove(&h, root, h.key(0), root, values[1], false);
    succeeds(&mut h, &[0], i); // Last-child removal needs no other child record.
    assert_eq!(
        page(&h, root, 0, 10),
        vec![sa(SOURCE), values[0], values[3], values[2]]
    );
}

#[test]
fn missing_readonly_or_forged_child_slots_and_missing_swap_child_roll_back() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let a = Address::new_from_array([70; 32]);
    let b = Address::new_from_array([71; 32]);
    for relative in [a, b] {
        let i = leaf(&h, root, h.key(0), root, relative, "Leaf", false);
        succeeds(&mut h, &[0], i);
    }
    let valid = h.indexed(remove(&h, root, h.key(0), root, a, false));
    let slot = sa(child_index_address(&ap(root), 1).0);
    for (target, readonly) in [(slot, false), (slot, true), (child(root, b), false)] {
        let mut invalid = valid.clone();
        if readonly {
            invalid
                .accounts
                .iter_mut()
                .find(|m| m.pubkey == target)
                .unwrap()
                .is_writable = false;
        } else {
            invalid.accounts.retain(|m| m.pubkey != target);
        }
        let before: Vec<_> = valid
            .accounts
            .iter()
            .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
            .collect();
        let failure = run_raw(&mut h, &[0], invalid).expect_err("invalid index inputs must reject");
        for (key, old) in before {
            let mut current = h.svm.get_account(&key);
            if key == h.key(0) {
                current.as_mut().unwrap().lamports += failure.meta.fee;
            }
            assert_eq!(current, old, "partial child-index mutation at {key}");
        }
    }
    // Valid program bytes at the wrong slot address cannot supply that position.
    let mut forged = h.svm.get_account(&slot).unwrap();
    let mut entry = decode_child_data(&ap(slot), &ap(forged.owner), &forged.data).unwrap();
    entry.index = 2;
    anchor_lang::AnchorSerialize::serialize(&entry, &mut &mut forged.data[8..]).unwrap();
    let original = h.svm.get_account(&slot).unwrap();
    h.svm.set_account(slot, forged).unwrap();
    assert!(run_raw(&mut h, &[0], valid.clone()).is_err());
    h.svm.set_account(slot, original).unwrap();
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), a, b]);
    assert!(run_raw(&mut h, &[0], valid).is_ok());
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), b]);
}
