//! Maintained child slots and reverse indexes, matching Solidity subs/subIndex.
use super::*;
use ledger::ledger_storage as mapping;
use ledger::ledger_view::Reader;

#[test]
fn removed_group_can_receive_as_implicit_leaf_and_become_a_group_again() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let relative = h.key(1);
    let key = h.remember(root, relative);
    let create = group(&h, root, h.key(0), root, relative, true, &[]);
    succeeds(&mut h, &[0], create);
    let container = h.storage(key);
    let removal = remove(&h, root, h.key(0), root, relative, true);
    succeeds(&mut h, &[0], removal);
    let retired = h.svm.get_account(&container).unwrap();
    // No-op removal must not require an unrelated last-child container.
    let repeat = remove(&h, root, h.key(0), root, relative, true);
    succeeds(&mut h, &[0], repeat);
    for amount in [10, 15] {
        let call = transfer(
            &h,
            root,
            h.key(0),
            (root, sa(SOURCE)),
            (root, relative),
            amount,
            &[],
        );
        succeeds(&mut h, &[0], call);
    }
    assert_eq!(h.record(key).debit, 25);
    assert!(!h.record(key).registered);
    assert_eq!(h.svm.get_account(&container).unwrap(), retired);
    let register = leaf(&h, root, h.key(0), root, relative, "", false);
    succeeds(&mut h, &[0], register);
    assert_eq!(h.record(key).debit, 25);
    let burn = transfer(
        &h,
        root,
        h.key(0),
        (root, relative),
        (root, sa(SOURCE)),
        25,
        &[],
    );
    succeeds(&mut h, &[0], burn);
    let removal = remove(&h, root, h.key(0), root, relative, false);
    succeeds(&mut h, &[0], removal);
    let create = group(&h, root, h.key(0), root, relative, true, &[]);
    succeeds(&mut h, &[0], create);
    assert!(h.record(key).is_container());
    assert_eq!(h.record(key).debit, 0);
    assert_eq!(h.storage(key), container);
    assert_eq!(
        h.svm.get_account(&container).unwrap().lamports,
        retired.lamports
    );
    let nested = leaf(&h, root, h.key(0), key, h.key(2), "Nested", false);
    succeeds(&mut h, &[0], nested);
    assert_eq!(h.record(key).children, 1);
}

fn page(h: &Harness, root: Address, start: u32, limit: u32) -> Vec<Address> {
    let mut reader = Reader::new();
    let record = h.account(&h.storage(root)).unwrap();
    reader
        .insert(ap(h.storage(root)), &ap(record.owner), &record.data)
        .unwrap();
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
    let last_key = h.storage(root);
    let last = h.account(&last_key).unwrap();
    assert_eq!(mapping::child(&last.data, 4).unwrap(), Some(ap(SYSTEM)));
    let i = leaf(&h, root, h.key(0), root, values[1], "Leaf", false);
    succeeds(&mut h, &[0], i);
    assert_eq!(h.account(&last_key).unwrap().lamports, last.lamports);
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
fn missing_readonly_or_forged_parent_container_rolls_back() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let a = Address::new_from_array([70; 32]);
    let b = Address::new_from_array([71; 32]);
    for relative in [a, b] {
        let i = leaf(&h, root, h.key(0), root, relative, "Leaf", false);
        succeeds(&mut h, &[0], i);
    }
    let valid = h.indexed(remove(&h, root, h.key(0), root, a, false));
    let slot = h.storage(root);
    for (target, readonly) in [(slot, false), (slot, true)] {
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
            .map(|m| (m.pubkey, h.account(&m.pubkey)))
            .collect();
        let failure = run_raw(&mut h, &[0], invalid).expect_err("invalid index inputs must reject");
        for (key, old) in before {
            let mut current = h.account(&key);
            if key == h.key(0) {
                current.as_mut().unwrap().lamports += failure.meta.fee;
            }
            assert_eq!(current, old, "partial child-index mutation at {key}");
        }
    }
    // Valid program bytes at the wrong slot address cannot supply that position.
    let mut forged = h.account(&slot).unwrap();
    mapping::set_child(&mut forged.data, 1, Some(ap(b))).unwrap();
    let original = h.account(&slot).unwrap();
    h.svm.set_account(slot, forged).unwrap();
    assert!(run_raw(&mut h, &[0], valid.clone()).is_err());
    h.svm.set_account(slot, original).unwrap();
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), a, b]);
    assert!(run_raw(&mut h, &[0], valid).is_ok());
    assert_eq!(page(&h, root, 0, 10), vec![sa(SOURCE), b]);
}
