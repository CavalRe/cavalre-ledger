//! Decode actual Ledger sBPF logs, checking provenance, order and transaction status.
use super::*;
use anchor_lang::Event;
use base64::{engine::general_purpose::STANDARD, Engine};
use ledger::ledger::{
    Credit, Debit, LedgerAdded, SubAccountAdded, SubAccountGroupAdded, SubAccountGroupRemoved,
    SubAccountRemoved,
};

// Logs can include other programs' events. Track invocation nesting instead of
// assuming every Program data line was emitted by Ledger.
fn event_bytes(logs: &[String]) -> Vec<Vec<u8>> {
    let mut stack = Vec::new();
    let mut events = Vec::new();
    let ledger = ledger::ID.to_string();
    for line in logs {
        if let Some(rest) = line.strip_prefix("Program ") {
            if let Some((program, _)) = rest.split_once(" invoke [") {
                stack.push(program.to_owned());
            } else if rest.ends_with(" success") || rest.contains(" failed:") {
                stack.pop();
            }
        }
        if stack.last() == Some(&ledger) {
            if let Some(data) = line.strip_prefix("Program data: ") {
                events.push(STANDARD.decode(data).unwrap());
            }
        }
    }
    events
}
fn credit(root: Address, account: Address, amount: u128, balance: u128) -> Vec<u8> {
    Credit {
        ledger: ap(root),
        account: ap(account),
        amount,
        balance,
    }
    .data()
}
fn debit(root: Address, account: Address, amount: u128, balance: u128) -> Vec<u8> {
    Debit {
        ledger: ap(root),
        account: ap(account),
        amount,
        balance,
    }
    .data()
}
fn created(root: Address, scope: Address, identifier: Address, name: &str) -> Vec<Vec<u8>> {
    vec![
        SubAccountAdded {
            ledger: ap(root),
            parent: ap(root),
            relative: SOURCE,
            is_credit: true,
        }
        .data(),
        LedgerAdded {
            ledger: ap(root),
            scope: ap(scope),
            identifier: ap(identifier),
            name: name.into(),
        }
        .data(),
    ]
}
fn assert_events(h: &mut Harness, signers: &[usize], ix: Instruction, expected: Vec<Vec<u8>>) {
    let result = run(h, signers, ix).unwrap(); // Only successful transactions are indexed.
    assert!(!result.logs.iter().any(|s| s.contains("truncated")));
    assert_eq!(event_bytes(&result.logs), expected);
}

#[test]
fn initialization_and_account_lifecycle_emit_original_event_families() {
    let mut h = Harness::new();
    let scope = h.key(0);
    let identifier = h.key(2);
    let root = sa(ledger::ledger_lib::root_address(&ap(scope), &ap(identifier)).0);
    let i = ix(
        h.registration(0, root),
        instruction::AddLedger {
            id: ap(identifier),
            name: "Scale".into(),
        },
        &[child(root, sa(SOURCE))],
    );
    assert_events(&mut h, &[0], i, created(root, scope, identifier, "Scale"));
    let app = child(root, scope);
    let add_group = group(&h, root, scope, root, scope, true, &[]);
    assert_events(
        &mut h,
        &[0],
        add_group.clone(),
        vec![SubAccountGroupAdded {
            ledger: ap(root),
            parent: ap(root),
            relative: ap(scope),
            name: "Group".into(),
            is_credit: false,
        }
        .data()],
    );
    assert_events(&mut h, &[0], add_group, vec![]);
    let user = h.key(1);
    let add_leaf = leaf(&h, root, scope, app, user, "User", false);
    assert_events(
        &mut h,
        &[0],
        add_leaf.clone(),
        vec![SubAccountAdded {
            ledger: ap(root),
            parent: ap(app),
            relative: ap(user),
            is_credit: false,
        }
        .data()],
    );
    assert_events(&mut h, &[0], add_leaf, vec![]);
    let remove_leaf = remove(&h, root, scope, app, user, false);
    assert_events(
        &mut h,
        &[0],
        remove_leaf.clone(),
        vec![SubAccountRemoved {
            ledger: ap(root),
            parent: ap(app),
            relative: ap(user),
        }
        .data()],
    );
    assert_events(&mut h, &[0], remove_leaf, vec![]);
    let remove_group = remove(&h, root, scope, root, scope, true);
    assert_events(
        &mut h,
        &[0],
        remove_group.clone(),
        vec![SubAccountGroupRemoved {
            ledger: ap(root),
            parent: ap(root),
            relative: ap(scope),
        }
        .data()],
    );
    assert_events(&mut h, &[0], remove_group, vec![]);

    // Native SOL has a root distinct from its zero asset identity.
    let native = sa(ledger::ledger_lib::NATIVE_SOL);
    let root = sa(ledger::ledger_lib::root_address(&ap(SYSTEM), &ap(native)).0);
    let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"vault", root.as_ref()],
        &ledger::ID,
    )
    .0);
    let i = ix(
        accounts::RegisterSol {
            global_root: ledger::ledger_lib::global_root_address().0,
            payer: ap(scope),
            root: ap(root),
            vault: ap(vault),
            system_program: ap(SYSTEM),
        },
        instruction::AddNativeSol {},
        &[child(root, sa(SOURCE))],
    );
    assert_events(&mut h, &[0], i, created(root, SYSTEM, native, "SOL"));
}

#[test]
fn custody_and_transfer_events_decode_through_direct_and_application_cpi_calls() {
    for program in [TOKEN, TOKEN_2022] {
        for cpi in [false, true] {
            let mut h = Harness::new();
            let e = External::setup(&mut h, 60, 1, program);
            let registration = e.registration(&h, "Token");
            assert_events(
                &mut h,
                &[0],
                registration,
                created(e.root, SYSTEM, e.mint, "Token"),
            );
            let consumer = Address::new_from_array([62; 32]);
            let authority = if cpi {
                h.svm
                    .add_program(
                        consumer,
                        &std::fs::read(
                            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                                .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                sa(anchor_lang::prelude::Pubkey::find_program_address(
                    &[b"app", h.key(0).as_ref()],
                    &ap(consumer),
                )
                .0)
            } else {
                h.key(0)
            };
            let call = |h: &Harness, i| {
                if cpi {
                    proxy(h, consumer, authority, i)
                } else {
                    i
                }
            };
            let parent = child(e.root, authority);
            let user = h.key(2);
            let other = Address::new_from_array([63; 32]);
            let a = child(parent, user);
            let b = child(parent, other);
            let i = call(
                &h,
                group(&h, e.root, authority, e.root, authority, true, &[]),
            );
            succeeds(&mut h, &[0], i);
            let i = call(
                &h,
                e.movement(&h, (authority, 1), (parent, user), 100, true, &[]),
            );
            assert_events(
                &mut h,
                &[0, 1],
                i,
                vec![
                    debit(e.root, a, 100, 100),
                    credit(e.root, e.source, 100, 100),
                    debit(e.root, parent, 100, 100),
                    credit(e.root, e.root, 100, 100),
                    debit(e.root, e.root, 100, 100),
                ],
            );
            assert!(!h.record(a).registered);
            let i = call(
                &h,
                transfer(
                    &h,
                    e.root,
                    authority,
                    (parent, user),
                    (parent, other),
                    40,
                    &[],
                ),
            );
            assert_events(
                &mut h,
                &[0],
                i,
                vec![credit(e.root, a, 40, 60), debit(e.root, b, 40, 40)],
            );
            let i = call(
                &h,
                transfer(
                    &h,
                    e.root,
                    authority,
                    (parent, user),
                    (parent, other),
                    0,
                    &[],
                ),
            );
            assert_events(
                &mut h,
                &[0],
                i,
                vec![credit(e.root, a, 0, 60), debit(e.root, b, 0, 40)],
            );
            let i = call(
                &h,
                transfer(
                    &h,
                    e.root,
                    authority,
                    (parent, user),
                    (parent, user),
                    60,
                    &[],
                ),
            );
            assert_events(&mut h, &[0], i, vec![]);
            let i = call(
                &h,
                e.movement(&h, (authority, 0), (parent, other), 40, false, &[]),
            );
            assert_events(
                &mut h,
                &[0],
                i,
                vec![
                    credit(e.root, b, 40, 0),
                    credit(e.root, parent, 40, 60),
                    debit(e.root, e.source, 40, 60),
                    credit(e.root, e.root, 40, 60),
                    debit(e.root, e.root, 40, 60),
                ],
            );
        }
    }
}

#[test]
fn failed_transactions_retain_speculative_logs_but_commit_no_event_effects() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let parent = branch(&mut h, e.root, 0, true);
    let user = h.key(1);
    let valid = e.movement(&h, (h.key(0), 0), (parent, user), 10, true, &[]);
    let before_root = h.svm.get_account(&e.root).unwrap();
    let before_parent = h.svm.get_account(&parent).unwrap();
    let mut late_failure = valid.clone();
    for account in &mut late_failure.accounts {
        if account.pubkey == parent {
            account.is_writable = false;
        }
    }
    let failed = run(&mut h, &[0], late_failure).unwrap_err();
    assert_eq!(
        failed.err,
        TransactionError::InstructionError(
            0,
            InstructionError::Custom(LedgerError::InvalidAccount.into())
        )
    );
    assert_eq!(
        event_bytes(&failed.meta.logs).len(),
        5,
        "Solana retains logs emitted before a late commit failure"
    );
    assert_eq!(h.svm.get_account(&e.root).unwrap(), before_root);
    assert_eq!(h.svm.get_account(&parent).unwrap(), before_parent);
    assert!(h.svm.get_account(&child(parent, user)).is_none());
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (1000, 0));

    // Even a successful Ledger instruction is rolled back by a later instruction.
    let fail = anchor_lang::solana_program::system_instruction::transfer(
        &ap(h.key(0)),
        &ap(h.key(2)),
        u64::MAX,
    );
    let tx = Transaction::new_signed_with_payer(
        &[valid, fail],
        Some(&h.key(0)),
        &[&h.keys[0]],
        h.svm.latest_blockhash(),
    );
    let failed = h.svm.send_transaction(tx).unwrap_err();
    assert!(matches!(
        failed.err,
        TransactionError::InstructionError(1, _)
    ));
    assert_eq!(event_bytes(&failed.meta.logs).len(), 5);
    assert_eq!(h.svm.get_account(&e.root).unwrap(), before_root);
    assert_eq!(h.svm.get_account(&parent).unwrap(), before_parent);
    assert!(h.svm.get_account(&child(parent, user)).is_none());
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (1000, 0));
}
