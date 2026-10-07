//! Minimal client write declarations, checked against actual sBPF execution.
use super::*;
use ledger::{ledger_lib::Child, ledger_view::Reader};

fn endpoint(parent: Address, relative: Address) -> Child {
    Child {
        parent: ap(parent),
        relative: ap(relative),
    }
}

fn planned(h: &Harness, mut ix: Instruction) -> Instruction {
    let mut reader = Reader::new();
    let mut seen = std::collections::BTreeSet::new();
    // Transfer has authority, ledger, source, destination, then ancestry/vault.
    for meta in ix.accounts.iter().skip(1).filter(|m| m.pubkey != SYSTEM) {
        if !seen.insert(meta.pubkey) {
            continue;
        }
        match h.svm.get_account(&meta.pubkey) {
            Some(account) if account.owner == sa(ledger::ID) => reader
                .insert(ap(meta.pubkey), &ap(account.owner), &account.data)
                .unwrap(),
            Some(_) => continue,
            None => reader.insert_missing(ap(meta.pubkey)).unwrap(),
        }
    }
    let data = &ix.data;
    let key = |offset| Address::new_from_array(data[offset..offset + 32].try_into().unwrap());
    let writable = reader
        .transfer_writable_accounts(
            &ap(ix.accounts[1].pubkey),
            endpoint(key(8), key(40)),
            endpoint(key(72), key(104)),
            u128::from_le_bytes(data[136..152].try_into().unwrap()),
        )
        .unwrap();
    for meta in ix.accounts.iter_mut().skip(1) {
        meta.is_writable = writable.contains(&ap(meta.pubkey));
    }
    ix
}

fn readonly(ix: &mut Instruction, key: Address) {
    for meta in &mut ix.accounts {
        if meta.pubkey == key {
            meta.is_writable = false;
        }
    }
}

#[test]
fn zero_and_self_transfers_require_registered_endpoints_and_only_charge_fees() {
    use anchor_lang::Event;
    use ledger::ledger::{Credit, Debit};
    let mut h = Harness::new();
    let e = External::new(&mut h, 210, 0);
    let app = branch(&mut h, e.root, 0, true);
    let a = h.key(1);
    let b = h.key(2);
    let from = child(app, a);
    let to = child(app, b);
    let missing = transfer(&h, e.root, h.key(0), (app, a), (app, b), 0, &[]);
    assert!(run_raw(&mut h, &[0], missing).is_err());
    assert!(h.svm.get_account(&from).is_none());
    assert!(h.svm.get_account(&to).is_none());
    for relative in [a, b] {
        let create = leaf(&h, e.root, h.key(0), app, relative, "", false);
        succeeds(&mut h, &[0], create);
    }
    let parent_before = [e.root_storage, app].map(|key| h.svm.get_account(&key));
    for receiver in [b, a] {
        // Construct the intended read-only transaction independently of the
        // planner, so a regression in both cannot hide unnecessary allocation.
        let mut ix = transfer(&h, e.root, h.key(0), (app, a), (app, receiver), 0, &[]);
        for meta in ix.accounts.iter_mut().skip(1) {
            meta.is_writable = false;
        }
        let payer_before = h.svm.get_account(&h.key(0)).unwrap().lamports;
        let result = run(&mut h, &[0], ix).unwrap();
        assert_eq!(
            h.svm.get_account(&h.key(0)).unwrap().lamports + result.fee,
            payer_before
        );
        assert_eq!(
            events::event_bytes(&result.logs),
            if receiver == a {
                vec![]
            } else {
                vec![
                    Credit {
                        ledger: ap(e.root),
                        account: ap(from),
                        amount: 0,
                        balance: 0,
                    }
                    .data(),
                    Debit {
                        ledger: ap(e.root),
                        account: ap(to),
                        amount: 0,
                        balance: 0,
                    }
                    .data(),
                ]
            }
        );
        assert_eq!(h.record(from).debit, 0);
        assert_eq!(h.record(to).debit, 0);
        assert_eq!(
            [e.root_storage, app].map(|key| h.svm.get_account(&key)),
            parent_before
        );
    }
    // The helper declares no writes for registered zero-balance endpoints.
    let ix = planned(
        &h,
        transfer(&h, e.root, h.key(0), (app, a), (app, b), 0, &[]),
    );
    check_plan(&mut h, ix, &[]);
    // A self-transfer still checks the public debit sender's available balance.
    let ix = planned(
        &h,
        transfer(&h, e.root, h.key(0), (app, a), (app, a), 1, &[]),
    );
    rejects(&mut h, &[0], ix, LedgerError::Accounting.into());
    // Zero amounts do not bypass custody or parent admission.
    let ix = planned(
        &h,
        transfer(&h, e.root, h.key(1), (app, a), (app, b), 0, &[]),
    );
    rejects(&mut h, &[0, 1], ix, LedgerError::Unauthorized.into());
    let closed = add_group(&mut h, e.root, app, 212, &[app]);
    // Create a separate registered-only group; the existing group's policy is immutable.
    let relative = Address::new_from_array([213; 32]);
    let create = group(&h, e.root, h.key(0), closed, relative, false, &[app]);
    succeeds(&mut h, &[0], create);
    let restricted = child(closed, relative);
    for relative in [a, b] {
        let create = crate::ix(
            h.base(0, e.root),
            instruction::CreateIdempotent {
                parent: ap(restricted),
                relative: ap(relative),
                bump: ledger::ledger_lib::to_address(&ledger::ID, &ap(restricted), &ap(relative)).1,
            },
            &[app, closed, restricted, child(restricted, relative)],
        );
        succeeds(&mut h, &[0], create);
    }
    let ix = planned(
        &h,
        transfer(
            &h,
            e.root,
            h.key(0),
            (restricted, a),
            (restricted, b),
            0,
            &[app, closed],
        ),
    );
    succeeds(&mut h, &[0], ix);
}

#[test]
fn zero_mint_burn_preserve_events_without_writing_registered_records() {
    use anchor_lang::Event;
    use ledger::ledger::{Credit, Debit};
    let mut h = Harness::new();
    let (root, source) = h.internal();
    let relative = h.key(1);
    let leaf = child(root, relative);
    let create = ix(
        h.base(0, root),
        instruction::CreateIdempotent {
            parent: ap(root),
            relative: ap(relative),
            bump: ledger::ledger_lib::to_address(&ledger::ID, &ap(root), &ap(relative)).1,
        },
        &[leaf],
    );
    succeeds(&mut h, &[0], create);
    for mint in [true, false] {
        let (from, to) = if mint {
            ((root, sa(SOURCE)), (root, relative))
        } else {
            ((root, relative), (root, sa(SOURCE)))
        };
        let ix = planned(&h, transfer(&h, root, h.key(0), from, to, 0, &[]));
        assert!(ix.accounts.iter().skip(1).all(|meta| !meta.is_writable));
        let before = [root, source].map(|key| h.svm.get_account(&key));
        let result = run(&mut h, &[0], ix).unwrap();
        let (from, to) = if mint { (source, leaf) } else { (leaf, source) };
        assert_eq!(
            events::event_bytes(&result.logs),
            vec![
                Credit {
                    ledger: ap(root),
                    account: ap(from),
                    amount: 0,
                    balance: 0
                }
                .data(),
                Debit {
                    ledger: ap(root),
                    account: ap(to),
                    amount: 0,
                    balance: 0
                }
                .data(),
                Credit {
                    ledger: ap(root),
                    account: ap(root),
                    amount: 0,
                    balance: 0
                }
                .data(),
                Debit {
                    ledger: ap(root),
                    account: ap(root),
                    amount: 0,
                    balance: 0
                }
                .data(),
            ]
        );
        assert_eq!(h.record(leaf).debit, 0);
        assert_eq!([root, source].map(|key| h.svm.get_account(&key)), before);
    }
}

#[test]
fn zero_token_settlement_keeps_absent_receiver_unallocated_and_checks_funder() {
    for token_program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let e = External::setup(&mut h, 220, 0, token_program);
        // Match the runtime's rent-exempt marker before comparing full account
        // snapshots; the generic token fixture starts with rent_epoch = 0.
        let mut wallet = h.svm.get_account(&e.wallet).unwrap();
        wallet.rent_epoch = u64::MAX;
        h.svm.set_account(e.wallet, wallet).unwrap();
        h.metadata(e.mint, "Token", "TOK");
        let register = e.registration(&h);
        succeeds(&mut h, &[0], register);
        let app = branch(&mut h, e.root, 0, true);
        let relative = h.key(2);
        let receiver = child(app, relative);
        let before =
            [e.root_storage, e.source, app, e.wallet, e.vault].map(|key| h.svm.get_account(&key));
        for deposit in [true, false] {
            let mut ix = e.movement(&h, (h.key(0), 0), (app, relative), 0, deposit, &[]);
            // Only the native custody wallet/vault and payer remain writable.
            for key in [e.root_storage, e.source, app, receiver] {
                readonly(&mut ix, key);
            }
            let payer_before = h.svm.get_account(&h.key(0)).unwrap().lamports;
            let result = run(&mut h, &[0], ix).unwrap();
            assert_eq!(
                h.svm.get_account(&h.key(0)).unwrap().lamports + result.fee,
                payer_before
            );
            assert!(h.svm.get_account(&receiver).is_none());
            assert_eq!(
                [e.root_storage, e.source, app, e.wallet, e.vault]
                    .map(|key| h.svm.get_account(&key)),
                before
            );
            assert_eq!(events::event_bytes(&result.logs).len(), 5);
        }
        let mut wrong_funder = e.movement(&h, (h.key(0), 1), (app, relative), 0, true, &[]);
        readonly(&mut wrong_funder, e.root_storage);
        rejects(
            &mut h,
            &[0, 1],
            wrong_funder,
            LedgerError::Unauthorized.into(),
        );
    }
}

#[test]
fn token_custody_requires_root_writes_only_for_nonzero_amounts_direct_and_cpi() {
    for token_program in [TOKEN, TOKEN_2022] {
        for mode in ["direct", "raw_cpi", "helper_cpi"] {
            let mut h = Harness::new();
            let e = External::setup(&mut h, 225, 0, token_program);
            let mut wallet = h.svm.get_account(&e.wallet).unwrap();
            wallet.rent_epoch = u64::MAX;
            h.svm.set_account(e.wallet, wallet).unwrap();
            h.metadata(e.mint, "Token", "TOK");
            let registration = e.registration(&h);
            succeeds(&mut h, &[0], registration);
            let app = Address::new_from_array([227; 32]);
            let authority = if mode != "direct" {
                h.svm
                    .add_program(
                        app,
                        &std::fs::read(
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                sa(anchor_lang::prelude::Pubkey::find_program_address(
                    &[b"app", h.key(0).as_ref()],
                    &ap(app),
                )
                .0)
            } else {
                h.key(0)
            };
            let call = |h: &Harness, ix| match mode {
                "helper_cpi" => custody_proxy(h, app, authority, ix),
                "raw_cpi" => proxy(h, app, authority, ix),
                _ => ix,
            };
            let parent = child(e.root, authority);
            let create = call(
                &h,
                group(&h, e.root, authority, e.root, authority, true, &[]),
            );
            succeeds(&mut h, &[0], create);
            let relative = h.key(2);
            let receiver = child(parent, relative);
            for deposit in [true, false] {
                let movement = e.movement(&h, (authority, 0), (parent, relative), 17, deposit, &[]);
                let mut missing_write = movement.clone();
                readonly(&mut missing_write, e.root_storage);
                let missing_write = call(&h, missing_write);
                // Real token movement and any leaf allocation must roll back
                // when the eventual Ledger root write cannot be committed.
                if mode == "helper_cpi" {
                    // The helper requests the missing write in its inner CPI;
                    // Solana must reject escalation beyond the outer privileges.
                    rejects_with_error(
                        &mut h,
                        &[0],
                        missing_write,
                        InstructionError::PrivilegeEscalation,
                    );
                } else {
                    rejects(
                        &mut h,
                        &[0],
                        missing_write,
                        LedgerError::InvalidAccount.into(),
                    );
                }
                let keys = [
                    e.root_storage,
                    e.source,
                    parent,
                    receiver,
                    e.vault,
                    e.wallet,
                ];
                let before = keys.map(|key| h.svm.get_account(&key));
                let mut zero = e.movement(&h, (authority, 0), (parent, relative), 0, deposit, &[]);
                for key in [e.root_storage, e.source, parent, receiver] {
                    readonly(&mut zero, key);
                }
                let zero = call(&h, zero);
                let payer_before = h.svm.get_account(&h.key(0)).unwrap().lamports;
                let result = run(&mut h, &[0], zero).unwrap();
                assert_eq!(
                    h.svm.get_account(&h.key(0)).unwrap().lamports + result.fee,
                    payer_before
                );
                assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);
                assert_eq!(events::event_bytes(&result.logs).len(), 5);
                let movement = call(&h, movement);
                let result = run(&mut h, &[0], movement).unwrap();
                eprintln!(
                    "token custody {token_program} {mode} deposit={deposit}: {} CUs",
                    result.compute_units_consumed
                );
                assert_eq!(h.record(e.root).credit, if deposit { 17 } else { 0 });
            }
            if mode == "helper_cpi" {
                let wrong_funder = call(
                    &h,
                    e.movement(&h, (authority, 1), (parent, relative), 0, true, &[]),
                );
                rejects(
                    &mut h,
                    &[0, 1],
                    wrong_funder,
                    LedgerError::Unauthorized.into(),
                );
            }
        }
    }
}

fn check_plan(h: &mut Harness, ix: Instruction, expected: &[Address]) {
    let mut actual: Vec<_> = ix
        .accounts
        .iter()
        .skip(1)
        .filter(|m| m.is_writable)
        .map(|m| m.pubkey)
        .collect();
    let mut expected = expected.to_vec();
    actual.sort();
    actual.dedup();
    expected.sort();
    assert_eq!(actual, expected, "wrong write set");
    let unchanged: Vec<_> = ix
        .accounts
        .iter()
        .skip(1)
        .filter(|m| !m.is_writable)
        .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
        .collect();
    // Every declared Ledger write is necessary. A missing permission must undo
    // earlier allocations and writes, not merely fail for an unrelated reason.
    for key in &expected {
        let mut missing = ix.clone();
        readonly(&mut missing, *key);
        rejects(h, &[0], missing, LedgerError::InvalidAccount.into());
    }
    succeeds(h, &[0], ix);
    for (key, before) in unchanged {
        assert_eq!(h.svm.get_account(&key), before, "read-only account changed");
    }
}

fn add_group(
    h: &mut Harness,
    root: Address,
    parent: Address,
    tag: u8,
    extra: &[Address],
) -> Address {
    let relative = Address::new_from_array([tag; 32]);
    let ix = group(h, root, h.key(0), parent, relative, true, extra);
    succeeds(h, &[0], ix);
    child(parent, relative)
}

#[test]
fn debit_paths_stop_below_common_ancestor_at_equal_and_unequal_depths() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 170, 0);
    let app = branch(&mut h, e.root, 0, true);
    let left = add_group(&mut h, e.root, app, 172, &[app]);
    let deep = add_group(&mut h, e.root, left, 173, &[app]);
    let right = add_group(&mut h, e.root, app, 174, &[app]);
    let a = h.key(1);
    let b = h.key(2);
    let deposit = e.movement(&h, (h.key(0), 0), (deep, a), 100, true, &[left, app]);
    succeeds(&mut h, &[0], deposit);

    let ix = planned(
        &h,
        transfer(
            &h,
            e.root,
            h.key(0),
            (deep, a),
            (right, b),
            30,
            &[left, app],
        ),
    );
    check_plan(
        &mut h,
        ix,
        &[child(deep, a), deep, left, child(right, b), right],
    );
    assert_eq!(h.record(left).debit, 70);
    assert_eq!(h.record(right).debit, 30);
    assert_eq!(h.record(app).debit, 100);
    assert_eq!(h.record(e.root).debit, 100);
    assert!(h.record(child(right, b)).child_index > 0);

    let ix = planned(
        &h,
        transfer(&h, e.root, h.key(0), (deep, a), (deep, b), 10, &[left, app]),
    );
    check_plan(&mut h, ix, &[child(deep, a), child(deep, b), deep]);
    assert_eq!(h.record(deep).debit, 70);
    assert_eq!(h.record(child(deep, a)).debit, 60);
    assert_eq!(h.record(child(deep, b)).debit, 10);

    // Events for zero amounts and self-transfer checks need no data writes.
    for (to, amount) in [((deep, b), 0), ((deep, a), 20)] {
        let ix = planned(
            &h,
            transfer(&h, e.root, h.key(0), (deep, a), to, amount, &[left, app]),
        );
        check_plan(&mut h, ix, &[]);
    }
}

#[test]
fn credit_paths_use_effective_leaf_polarity_and_leave_app_and_ledger_readonly() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let app = add_group(&mut h, root, root, 180, &[]);
    let relative = Address::new_from_array([181; 32]);
    let credit_group = child(app, relative);
    let ix = ix(
        h.base(0, root),
        instruction::AddSubAccountGroup {
            parent: ap(app),
            relative: ap(relative),
            name: "Credit".into(),
            credit: true,
        },
        &[app, credit_group],
    );
    succeeds(&mut h, &[0], ix);
    let a = h.key(1);
    let b = h.key(2);
    // Implicit credit inherits its credit parent. The app above it is a debit group.
    let mint = planned(
        &h,
        transfer(&h, root, h.key(0), (credit_group, a), (app, a), 100, &[app]),
    );
    check_plan(
        &mut h,
        mint,
        &[
            child(credit_group, a),
            credit_group,
            child(app, a),
            app,
            root,
        ],
    );
    // Explicit credit overrides its debit parent's polarity.
    let register = leaf(&h, root, h.key(0), app, b, "Credit leaf", true);
    succeeds(&mut h, &[0], register);
    let ix = planned(
        &h,
        transfer(&h, root, h.key(0), (app, b), (credit_group, a), 25, &[app]),
    );
    check_plan(
        &mut h,
        ix,
        &[child(app, b), child(credit_group, a), credit_group],
    );
    assert_eq!(h.record(child(app, b)).credit, 25);
    assert_eq!(h.record(child(credit_group, a)).credit, 75);
    assert_eq!(h.record(app).credit, 100);
    assert_eq!(h.record(root).credit, 100);
    // Burn below the app still changes both columns all the way to the token root.
    let burn = planned(
        &h,
        transfer(&h, root, h.key(0), (app, a), (app, b), 10, &[]),
    );
    check_plan(&mut h, burn, &[child(app, a), child(app, b), app, root]);
    assert_eq!((h.record(root).debit, h.record(root).credit), (90, 90));
}

#[test]
fn cpi_preserves_readonly_common_ancestors() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 190, 0);
    let program = Address::new_from_array([192; 32]);
    let binary = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap();
    h.svm.add_program(program, &binary).unwrap();
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
        group(&h, e.root, authority, e.root, authority, true, &[]),
    );
    succeeds(&mut h, &[0], create);
    let a = h.key(1);
    let b = h.key(2);
    let deposit = proxy(
        &h,
        program,
        authority,
        e.movement(&h, (authority, 0), (app, a), 100, true, &[]),
    );
    succeeds(&mut h, &[0], deposit);
    let create = proxy(
        &h,
        program,
        authority,
        leaf(&h, e.root, authority, app, b, "", false),
    );
    succeeds(&mut h, &[0], create);
    let before = [e.root_storage, app].map(|key| h.svm.get_account(&key));
    let inner = planned(
        &h,
        transfer(&h, e.root, authority, (app, a), (app, b), 20, &[]),
    );
    for key in [e.root_storage, app] {
        assert!(
            !inner
                .accounts
                .iter()
                .find(|m| m.pubkey == key)
                .unwrap()
                .is_writable
        );
    }
    let outer = proxy(&h, program, authority, inner);
    succeeds(&mut h, &[0], outer);
    assert_eq!(
        [e.root_storage, app].map(|key| h.svm.get_account(&key)),
        before
    );
    assert_eq!(h.record(child(app, b)).debit, 20);
}

#[test]
fn tree_mutations_require_only_changed_ancestors_and_repeats_need_no_root_write() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let app = add_group(&mut h, root, root, 200, &[]);
    let root_before = h.svm.get_account(&root);
    let relative = h.key(1);
    let mut create = leaf(&h, root, h.key(0), app, relative, "Leaf", false);
    readonly(&mut create, root);
    succeeds(&mut h, &[0], create.clone());
    assert_eq!(h.svm.get_account(&root), root_before);
    // A matching repeat has no record writes or index changes.
    readonly(&mut create, app);
    readonly(&mut create, child(app, relative));
    succeeds(&mut h, &[0], create);
    let mut remove = remove(&h, root, h.key(0), app, relative, false);
    readonly(&mut remove, root);
    succeeds(&mut h, &[0], remove);
    assert_eq!(h.svm.get_account(&root), root_before);
    let mut direct = leaf(&h, root, h.key(0), root, relative, "Direct", false);
    readonly(&mut direct, root);
    rejects(&mut h, &[0], direct, LedgerError::InvalidAccount.into());
}
