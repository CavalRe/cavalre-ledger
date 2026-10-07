//! Existing-account transfers exercise the lean entrypoint against real sBPF.
use super::*;
use ledger::ledger_lib::Child;
pub(super) fn fast(
    h: &Harness,
    root: Address,
    authority: Address,
    from: (Address, Address),
    to: (Address, Address),
    amount: u128,
    rest: &[Address],
) -> Instruction {
    let mut metas = Vec::new();
    for key in rest {
        metas.push(
            anchor_lang::solana_program::instruction::AccountMeta::new_readonly(ap(*key), false),
        );
    }
    let call = ledger::ledger_transfer::instruction(
        ap(authority),
        ap(root),
        Child {
            parent: ap(from.0),
            relative: ap(from.1),
        },
        Child {
            parent: ap(to.0),
            relative: ap(to.1),
        },
        amount,
        &metas,
    );
    let _ = h;
    Instruction {
        program_id: sa(call.program_id),
        data: call.data,
        accounts: call
            .accounts
            .into_iter()
            .map(|a| AccountMeta {
                pubkey: sa(a.pubkey),
                is_signer: a.is_signer,
                is_writable: a.is_writable,
            })
            .collect(),
    }
}
#[test]
fn custodian_transfers_use_only_endpoints_ledger_and_vault() {
    for token_program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 130, 0, token_program);
        h.metadata(e.mint, "Token", "TOK");
        let registration = e.registration(&h);
        succeeds(&mut h, &[0], registration);
        let alice = h.key(0);
        let bob = h.key(1);
        for relative in [alice, bob] {
            let deposit = e.movement(&h, (relative, 0), (e.root, relative), 100, true, &[]);
            succeeds(
                &mut h,
                if relative == alice { &[0] } else { &[0, 1] },
                deposit,
            );
        }
        let before = [e.root, e.vault].map(|key| h.svm.get_account(&key));
        let call = fast(
            &h,
            e.root,
            alice,
            (e.root, alice),
            (e.root, bob),
            7,
            &[e.vault],
        );
        let result = run_raw(&mut h, &[0], call.clone()).unwrap();
        eprintln!(
            "PACKED_CUSTODIAN_TRANSFER_CU={} TOKEN_PROGRAM={token_program}",
            result.compute_units_consumed
        );
        assert!(
            result.compute_units_consumed <= 2_150,
            "existing custodian transfer CU regression"
        );
        assert_eq!(h.record(child(e.root, alice)).debit, 93);
        assert_eq!(h.record(child(e.root, bob)).debit, 107);
        assert_eq!([e.root, e.vault].map(|key| h.svm.get_account(&key)), before);
        assert_eq!(events::event_bytes(&result.logs).len(), 2);
        for altered in 0..5 {
            let mut bad = call.clone();
            match altered {
                0 => bad.data[152] ^= 1,
                1 => bad.data[40] ^= 1,
                2 => bad.accounts[0].pubkey = bob,
                3 => bad.accounts[2].is_writable = false,
                _ => bad.accounts[3].pubkey = e.source,
            }
            let balances =
                [child(e.root, alice), child(e.root, bob)].map(|key| h.record(key).debit);
            assert!(run_raw(&mut h, if altered == 2 { &[0, 1] } else { &[0] }, bad).is_err());
            assert_eq!(
                [child(e.root, alice), child(e.root, bob)].map(|key| h.record(key).debit),
                balances
            );
        }
        let self_transfer = fast(
            &h,
            e.root,
            alice,
            (e.root, alice),
            (e.root, alice),
            94,
            &[e.vault],
        );
        assert!(run_raw(&mut h, &[0], self_transfer).is_err());
        let original_vault = h.svm.get_account(&e.vault).unwrap();
        for invalid in 0..if token_program == TOKEN_2022 { 6 } else { 3 } {
            let mut vault = original_vault.clone();
            match invalid {
                0 => vault.data[64..72].copy_from_slice(&0u64.to_le_bytes()),
                1 => vault.data[0] ^= 1,
                2 => vault.data[108] = 0,
                _ => vault.data[[72, 109, 129][invalid - 3]] = 2,
            }
            h.svm.set_account(e.vault, vault).unwrap();
            let before =
                [child(e.root, alice), child(e.root, bob)].map(|key| h.svm.get_account(&key));
            assert!(run_raw(&mut h, &[0], call.clone()).is_err());
            assert_eq!(
                [child(e.root, alice), child(e.root, bob)].map(|key| h.svm.get_account(&key)),
                before
            );
        }
        h.svm.set_account(e.vault, original_vault).unwrap();
    }
}
#[test]
fn sibling_transfers_at_depth_four_leave_parent_unchanged() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 134, 0);
    let app = branch(&mut h, e.root, 0, true);
    let alice = h.key(0);
    let bob = h.key(1);
    for relative in [alice, bob] {
        let deposit = e.movement(&h, (alice, 0), (app, relative), 100, true, &[]);
        succeeds(&mut h, &[0], deposit);
    }
    let before = [e.root, app, e.vault].map(|key| h.svm.get_account(&key));
    let call = fast(
        &h,
        e.root,
        alice,
        (app, alice),
        (app, bob),
        7,
        &[app, e.vault],
    );
    let result = run_raw(&mut h, &[0], call).unwrap();
    eprintln!(
        "PACKED_DEPTH4_SIBLING_TRANSFER_CU={}",
        result.compute_units_consumed
    );
    assert!(
        result.compute_units_consumed <= 2_250,
        "existing sibling transfer CU regression"
    );
    assert_eq!(h.record(child(app, alice)).debit, 93);
    assert_eq!(h.record(child(app, bob)).debit, 107);
    assert_eq!(
        [e.root, app, e.vault].map(|key| h.svm.get_account(&key)),
        before
    );
}

// Compare conservative and minimal account declarations against the same
// pre-state, including every event and ancestor balance. There is one handler.
fn parity(
    h: &mut Harness,
    ledger: Address,
    from: (Address, Address),
    to: (Address, Address),
    amount: u128,
    writable: &[Address],
    readonly: &[Address],
) {
    let mut rest = writable.to_vec();
    rest.extend_from_slice(readonly);
    let normal = h.indexed(transfer(h, ledger, h.key(0), from, to, amount, &rest));
    let snapshots: Vec<_> = normal
        .accounts
        .iter()
        .filter_map(|a| {
            h.svm
                .get_account(&a.pubkey)
                .filter(|state| state.owner == sa(ledger::ID))
                .map(|state| (a.pubkey, state))
        })
        .collect();
    let expected = run_raw(h, &[0], normal).unwrap();
    let after: Vec<_> = snapshots
        .iter()
        .map(|(key, _)| (*key, h.svm.get_account(key)))
        .collect();
    for (key, state) in &snapshots {
        h.svm.set_account(*key, state.clone()).unwrap();
    }
    let mut lean = fast(h, ledger, h.key(0), from, to, amount, &rest);
    for meta in &mut lean.accounts {
        meta.is_writable |= writable.contains(&meta.pubkey);
    }
    let result = run_raw(h, &[0], lean).unwrap();
    assert_eq!(
        events::event_bytes(&result.logs),
        events::event_bytes(&expected.logs)
    );
    for (key, state) in after {
        assert_eq!(h.svm.get_account(&key), state, "state diverged at {key}");
    }
    for key in readonly {
        if let Some((_, before)) = snapshots.iter().find(|(k, _)| k == key) {
            assert_eq!(h.svm.get_account(key), Some(before.clone()));
        }
    }
    eprintln!(
        "PACKED_WALK_CU={} CONSERVATIVE_CU={}",
        result.compute_units_consumed, expected.compute_units_consumed
    );
}

#[test]
fn ancestor_walk_preserves_events_with_minimal_writes_for_unequal_depths_and_polarities() {
    let mut h = Harness::new();
    let (ledger, source) = h.internal();
    let authority = h.key(0);
    let app = branch(&mut h, ledger, 0, true);
    let mut make_group = |parent, tag, rest: &[Address]| {
        let relative = Address::new_from_array([tag; 32]);
        let call = group(&h, ledger, authority, parent, relative, true, rest);
        succeeds(&mut h, &[0], call);
        child(parent, relative)
    };
    let left = make_group(app, 140, &[app]);
    let deep = make_group(left, 141, &[app]);
    let right = make_group(app, 142, &[app]);
    let a = h.key(1);
    let b = h.key(2);
    for (parent, rest) in [(deep, vec![left, app]), (right, vec![app])] {
        let call = transfer(
            &h,
            ledger,
            authority,
            (ledger, sa(SOURCE)),
            (parent, a),
            100,
            &rest,
        );
        succeeds(&mut h, &[0], call);
    }
    parity(
        &mut h,
        ledger,
        (deep, a),
        (right, a),
        7,
        &[deep, left, right],
        &[app],
    );
    let credit = leaf(&h, ledger, authority, app, b, "Credit", true);
    succeeds(&mut h, &[0], credit);
    // Issue through the common ancestor and the ledger, then move credit between
    // the application's credit leaf and Source without changing the ledger.
    parity(
        &mut h,
        ledger,
        (app, b),
        (deep, a),
        9,
        &[deep, left, app, ledger],
        &[],
    );
    parity(
        &mut h,
        ledger,
        (app, b),
        (ledger, sa(SOURCE)),
        11,
        &[app],
        &[],
    );
    assert_eq!(h.record(source).credit, 189);
}

#[test]
fn late_ancestor_overflow_rolls_back_every_lean_write() {
    let mut h = Harness::new();
    let (ledger, source) = h.internal();
    let authority = h.key(0);
    let a = h.key(1);
    let b = h.key(2);
    let mint = transfer(
        &h,
        ledger,
        authority,
        (ledger, sa(SOURCE)),
        (ledger, a),
        u128::MAX,
        &[],
    );
    succeeds(&mut h, &[0], mint);
    let create = leaf(&h, ledger, authority, ledger, b, "Recipient", false);
    succeeds(&mut h, &[0], create);
    let mut call = fast(
        &h,
        ledger,
        authority,
        (ledger, sa(SOURCE)),
        (ledger, b),
        1,
        &[],
    );
    call.accounts[1].is_writable = true;
    let keys = [ledger, source, child(ledger, a), child(ledger, b)];
    let before = keys.map(|key| h.svm.get_account(&key));
    assert!(run_raw(&mut h, &[0], call).is_err());
    assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);
}

#[test]
fn application_cpi_signer_can_use_the_lean_sibling_path() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 146, 0);
    let program = Address::new_from_array([148; 32]);
    h.svm
        .add_program(
            program,
            &std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
            )
            .unwrap(),
        )
        .unwrap();
    let authority = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"app", h.key(0).as_ref()],
        &ap(program),
    )
    .0);
    let app = child(e.root, authority);
    let create = proxy(
        &h,
        program,
        authority,
        h.indexed(group(&h, e.root, authority, e.root, authority, true, &[])),
    );
    run_raw(&mut h, &[0], create).unwrap();
    let a = h.key(1);
    let b = h.key(2);
    for relative in [a, b] {
        let deposit = proxy(
            &h,
            program,
            authority,
            h.indexed(e.movement(&h, (authority, 0), (app, relative), 100, true, &[])),
        );
        run_raw(&mut h, &[0], deposit).unwrap();
    }
    let before = [e.root, app, e.vault].map(|key| h.svm.get_account(&key));
    let call = proxy(
        &h,
        program,
        authority,
        fast(
            &h,
            e.root,
            authority,
            (app, a),
            (app, b),
            7,
            &[app, e.vault],
        ),
    );
    run_raw(&mut h, &[0], call).unwrap();
    assert_eq!(h.record(child(app, a)).debit, 93);
    assert_eq!(h.record(child(app, b)).debit, 107);
    assert_eq!(
        [e.root, app, e.vault].map(|key| h.svm.get_account(&key)),
        before
    );
}

#[test]
fn lean_parser_preserves_aliases_and_noops_and_rejects_substitute_layouts() {
    use ledger::ledger_storage as storage;
    let mut h = Harness::new();
    let e = External::new(&mut h, 152, 0);
    let alice = h.key(0);
    let bob = h.key(1);
    for relative in [alice, bob] {
        let deposit = e.movement(&h, (relative, 0), (e.root, relative), 100, true, &[]);
        succeeds(
            &mut h,
            if relative == alice { &[0] } else { &[0, 1] },
            deposit,
        );
    }
    let keys = [child(e.root, alice), child(e.root, bob)];
    let before = keys.map(|key| h.svm.get_account(&key).unwrap());
    let call = fast(
        &h,
        e.root,
        alice,
        (e.root, alice),
        (e.root, bob),
        7,
        &[e.vault],
    );
    let expected = run_raw(&mut h, &[0], call.clone()).unwrap();
    let after = keys.map(|key| h.svm.get_account(&key));
    for (key, state) in keys.into_iter().zip(&before) {
        h.svm.set_account(key, state.clone()).unwrap();
    }
    let mut aliases = call.clone();
    aliases.accounts.extend(call.accounts.iter().cloned());
    let result = run_raw(&mut h, &[0], aliases).unwrap();
    assert_eq!(
        events::event_bytes(&result.logs),
        events::event_bytes(&expected.logs)
    );
    assert_eq!(keys.map(|key| h.svm.get_account(&key)), after);

    // Two read-only aliases of the same account must be accepted as a funded
    // self-transfer. Distinct zero transfers retain both original posting logs.
    for (to, amount, event_count) in [(alice, 7, 0), (bob, 0, 2)] {
        let mut noop = fast(
            &h,
            e.root,
            alice,
            (e.root, alice),
            (e.root, to),
            amount,
            &[e.vault],
        );
        noop.accounts[2].is_writable = false;
        noop.accounts[3].is_writable = false;
        let result = run_raw(&mut h, &[0], noop).unwrap();
        assert_eq!(events::event_bytes(&result.logs).len(), event_count);
        assert_eq!(keys.map(|key| h.svm.get_account(&key)), after);
    }

    let original = h.svm.get_account(&keys[0]).unwrap();
    for invalid in 0..8 {
        let mut forged = original.clone();
        match invalid {
            0 => forged.data[storage::PARENT] ^= 1,
            1 => forged.data[storage::KIND] = 0,
            2 => forged.data[storage::DEPTH] = 2,
            3 => forged.data[storage::CHILD_INDEX..storage::CHILD_INDEX + 4].fill(0),
            4 => forged.data[storage::HEADER_LEN] = 1,
            5 => forged.data.push(0),
            6 => {
                forged.data.pop();
            }
            _ => forged.owner = SYSTEM,
        }
        h.svm.set_account(keys[0], forged.clone()).unwrap();
        assert!(run_raw(&mut h, &[0], call.clone()).is_err());
        assert_eq!(h.svm.get_account(&keys[0]), Some(forged));
        assert_eq!(h.svm.get_account(&keys[1]), after[1]);
    }
    h.svm.set_account(keys[0], original.clone()).unwrap();
    // Even same-owner, byte-identical data at the metadata namespace cannot
    // stand in for the accounting PDA.
    let metadata = sa(ledger::ledger_lib::metadata_address(&ap(e.root), &ap(alice)).0);
    h.svm.set_account(metadata, original).unwrap();
    let mut substitute = call.clone();
    substitute.accounts[2].pubkey = metadata;
    assert!(run_raw(&mut h, &[0], substitute).is_err());
    for len in [8, 153, 155] {
        let mut malformed = call.clone();
        malformed.data.resize(len, 0);
        assert!(run_raw(&mut h, &[0], malformed).is_err());
    }
    let mut missing = call;
    missing.accounts.truncate(3);
    assert!(run_raw(&mut h, &[0], missing).is_err());
    assert_eq!(keys.map(|key| h.svm.get_account(&key)), after);
}

#[test]
fn credit_siblings_use_the_same_posting_path_and_reject_overdrafts() {
    let mut h = Harness::new();
    let (ledger, _) = h.internal();
    let authority = h.key(0);
    let app = branch(&mut h, ledger, 0, true);
    let alice = h.key(1);
    let bob = h.key(2);
    for relative in [alice, bob] {
        let create = leaf(&h, ledger, authority, app, relative, "", true);
        succeeds(&mut h, &[0], create);
        let issue = transfer(
            &h,
            ledger,
            authority,
            (app, relative),
            (app, authority),
            100,
            &[app],
        );
        succeeds(&mut h, &[0], issue);
    }
    parity(&mut h, ledger, (app, alice), (app, bob), 7, &[], &[app]);
    assert_eq!(h.record(child(app, alice)).credit, 107);
    assert_eq!(h.record(child(app, bob)).credit, 93);
    let keys = [ledger, app, child(app, alice), child(app, bob)];
    let before = keys.map(|key| h.svm.get_account(&key));
    let overdraft = fast(&h, ledger, authority, (app, alice), (app, bob), 94, &[app]);
    assert!(run_raw(&mut h, &[0], overdraft).is_err());
    assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);
}
