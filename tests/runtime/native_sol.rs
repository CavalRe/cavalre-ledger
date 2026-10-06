//! Native lamport settlement through the real System Program and Ledger sBPF.
use super::*;
use anchor_lang::solana_program::system_instruction;
use ledger::ledger_lib::{TokenKind, NATIVE_SOL};

const AMOUNT: u64 = 100_000_000;
struct Sol {
    root: Address,
    root_storage: Address,
    source: Address,
    vault: Address,
}
impl Sol {
    fn addresses() -> Self {
        let root_storage = sa(ledger::ledger_lib::ledger_pda(&ap(SYSTEM), &NATIVE_SOL).0);
        let root = root_storage;
        Self {
            root,
            root_storage,
            source: child(root, sa(SOURCE)),
            vault: sa(anchor_lang::prelude::Pubkey::find_program_address(
                &[b"vault", root_storage.as_ref()],
                &ledger::ID,
            )
            .0),
        }
    }
    fn registration(&self, h: &Harness) -> Instruction {
        ix(
            accounts::RegisterSol {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                ledger: ap(self.root_storage),
                vault: ap(self.vault),
                system_program: ap(SYSTEM),
            },
            instruction::AddNativeSol {},
            &[self.source],
        )
    }
    fn new(h: &mut Harness) -> Self {
        h.external_root(sa(NATIVE_SOL));
        let sol = Self::addresses();
        succeeds(h, &[0], sol.registration(h));
        sol
    }
    fn custody(&self, h: &Harness) -> u64 {
        lamports(h, self.vault) - h.svm.minimum_balance_for_rent_exemption(0)
    }
    fn movement(
        &self,
        h: &Harness,
        auth: (Address, usize),
        wallet: Address,
        endpoint: (Address, Address),
        amount: u64,
        deposit: bool,
    ) -> Instruction {
        let accounts = accounts::MoveSol {
            payer: ap(h.key(0)),
            authority: ap(auth.0),
            funding_authority: ap(h.key(auth.1)),
            ledger: ap(self.root_storage),
            vault: ap(self.vault),
            wallet: ap(wallet),
            system_program: ap(SYSTEM),
        };
        let (parent, relative) = endpoint;
        let remaining = remaining(self.root, &[self.source, parent, child(parent, relative)]);
        if deposit {
            ix(
                accounts,
                instruction::WrapSol {
                    parent: ap(parent),
                    relative: ap(relative),
                    amount,
                },
                &remaining,
            )
        } else {
            ix(
                accounts,
                instruction::UnwrapSol {
                    parent: ap(parent),
                    relative: ap(relative),
                    amount,
                },
                &remaining,
            )
        }
    }
}
fn lamports(h: &Harness, address: Address) -> u64 {
    h.svm.get_account(&address).map_or(0, |a| a.lamports)
}

#[test]
fn zero_sol_settlement_never_allocates_receiver_storage() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let app = branch(&mut h, sol.root, 0, true);
    let receiver = h.key(2);
    let leaf = child(app, receiver);
    let before =
        [sol.root_storage, sol.source, app, sol.vault, h.key(1)].map(|key| h.svm.get_account(&key));
    for deposit in [true, false] {
        let mut ix = sol.movement(&h, (h.key(0), 1), h.key(1), (app, receiver), 0, deposit);
        for meta in &mut ix.accounts {
            if [sol.root_storage, sol.source, app, leaf].contains(&meta.pubkey) {
                meta.is_writable = false;
            }
        }
        let payer_before = lamports(&h, h.key(0));
        let result = run(&mut h, &[0, 1], ix).unwrap();
        assert_eq!(lamports(&h, h.key(0)) + result.fee, payer_before);
        assert!(h.svm.get_account(&leaf).is_none());
        assert_eq!(
            [sol.root_storage, sol.source, app, sol.vault, h.key(1)]
                .map(|key| h.svm.get_account(&key)),
            before
        );
        assert_eq!(super::events::event_bytes(&result.logs).len(), 5);
    }
}

#[test]
fn sol_custody_requires_root_writes_only_for_nonzero_amounts_direct_and_cpi() {
    for mode in ["direct", "raw_cpi", "helper_cpi"] {
        let mut h = Harness::new();
        let sol = Sol::new(&mut h);
        let app = Address::new_from_array([82; 32]);
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
        let parent = child(sol.root, authority);
        let create = call(
            &h,
            group(&h, sol.root, authority, sol.root, authority, true, &[]),
        );
        succeeds(&mut h, &[0], create);
        let receiver = h.key(2);
        let leaf = child(parent, receiver);
        for deposit in [true, false] {
            let movement = sol.movement(
                &h,
                (authority, 1),
                h.key(1),
                (parent, receiver),
                AMOUNT,
                deposit,
            );
            let mut missing_write = movement.clone();
            for meta in &mut missing_write.accounts {
                if meta.pubkey == sol.root_storage {
                    meta.is_writable = false;
                }
            }
            let missing_write = call(&h, missing_write);
            if mode == "helper_cpi" {
                rejects_with_error(
                    &mut h,
                    &[0, 1],
                    missing_write,
                    InstructionError::PrivilegeEscalation,
                );
            } else {
                rejects(
                    &mut h,
                    &[0, 1],
                    missing_write,
                    LedgerError::InvalidAccount.into(),
                );
            }
            let keys = [
                sol.root_storage,
                sol.source,
                parent,
                leaf,
                sol.vault,
                h.key(1),
            ];
            let before = keys.map(|key| h.svm.get_account(&key));
            let mut zero =
                sol.movement(&h, (authority, 1), h.key(1), (parent, receiver), 0, deposit);
            for meta in &mut zero.accounts {
                if [sol.root_storage, sol.source, parent, leaf].contains(&meta.pubkey) {
                    meta.is_writable = false;
                }
            }
            let zero = call(&h, zero);
            let payer_before = lamports(&h, h.key(0));
            let result = run(&mut h, &[0, 1], zero).unwrap();
            assert_eq!(lamports(&h, h.key(0)) + result.fee, payer_before);
            assert_eq!(keys.map(|key| h.svm.get_account(&key)), before);
            assert_eq!(super::events::event_bytes(&result.logs).len(), 5);
            let movement = call(&h, movement);
            let result = run(&mut h, &[0, 1], movement).unwrap();
            eprintln!(
                "SOL custody {mode} deposit={deposit}: {} CUs",
                result.compute_units_consumed
            );
            assert_eq!(sol.custody(&h), if deposit { AMOUNT } else { 0 });
        }
    }
}

#[test]
fn native_sol_round_trip_with_distinct_payer_and_unsigned_recipient_direct_and_cpi() {
    for cpi in [false, true] {
        let mut h = Harness::new();
        let sol = Sol::new(&mut h);
        assert_eq!(sol.custody(&h), 0);
        assert_eq!(h.record(sol.root).ledger.as_ref().unwrap().token_kind, 1);
        assert_eq!(h.record(sol.root).flags().token_kind, TokenKind::Native);
        let app = Address::new_from_array([80; 32]);
        let authority = if cpi {
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
        let call = |h: &Harness, i| if cpi { proxy(h, app, authority, i) } else { i };
        let parent = child(sol.root, authority);
        let a = h.key(2);
        let b = Address::new_from_array([81; 32]);
        let i = call(
            &h,
            group(&h, sol.root, authority, sol.root, authority, true, &[]),
        );
        succeeds(&mut h, &[0], i);
        let before = lamports(&h, h.key(1));
        let i = call(
            &h,
            sol.movement(&h, (authority, 1), h.key(1), (parent, a), AMOUNT, true),
        );
        succeeds(&mut h, &[0, 1], i);
        assert_eq!(before - lamports(&h, h.key(1)), AMOUNT);
        assert!(h.record(child(parent, a)).child_index > 0);
        let i = call(
            &h,
            transfer(
                &h,
                sol.root,
                authority,
                (parent, a),
                (parent, b),
                40_000_000,
                &[],
            ),
        );
        succeeds(&mut h, &[0], i);
        let recipient_before = lamports(&h, h.key(2));
        for (relative, amount) in [(b, 40_000_000), (a, 60_000_000)] {
            let i = call(
                &h,
                sol.movement(
                    &h,
                    (authority, 0),
                    h.key(2),
                    (parent, relative),
                    amount,
                    false,
                ),
            );
            succeeds(&mut h, &[0], i);
        }
        assert_eq!(lamports(&h, h.key(2)) - recipient_before, AMOUNT);
        assert_eq!(sol.custody(&h), 0);
        assert_eq!(h.record(sol.source).credit, 0);
        assert_eq!(
            (h.record(sol.root).debit, h.record(sol.root).credit),
            (0, 0)
        );
        assert_eq!(h.svm.get_account(&sol.vault).unwrap().owner, SYSTEM);
    }
}

#[test]
fn native_sol_fee_payer_can_also_fund_and_receive() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let payer = h.key(0);
    let before = lamports(&h, payer);
    let i = sol.movement(&h, (payer, 0), payer, (sol.root, payer), AMOUNT, true);
    let result = run(&mut h, &[0], i).unwrap();
    assert_eq!(
        before - lamports(&h, payer),
        AMOUNT
            + result.fee
            + h.svm
                .minimum_balance_for_rent_exemption(ledger::ledger_storage::LEAF_LEN)
            + h.svm.minimum_balance_for_rent_exemption(32)
            - h.svm.minimum_balance_for_rent_exemption(0)
    );
    assert_eq!(h.record(child(sol.root, payer)).debit, u128::from(AMOUNT));
    let before = lamports(&h, payer);
    let i = sol.movement(&h, (payer, 0), payer, (sol.root, payer), AMOUNT, false);
    let result = run(&mut h, &[0], i).unwrap();
    assert_eq!(lamports(&h, payer) + result.fee, before + AMOUNT);
    assert_eq!(sol.custody(&h), 0);
}

#[test]
fn native_sol_prefunding_and_donations_create_no_claims() {
    let mut h = Harness::new();
    let sol = Sol::addresses();
    h.external_root(sol.root);
    // Failure after funding the new vault and allocating the root must undo both.
    let mut failed = sol.registration(&h);
    for account in &mut failed.accounts {
        if account.pubkey == sol.source {
            account.is_writable = false;
        }
    }
    rejects(&mut h, &[0], failed, LedgerError::InvalidAccount.into());
    for address in [sol.root_storage, sol.source, sol.vault] {
        assert!(h.svm.get_account(&address).is_none());
    }
    let reserve = h.svm.minimum_balance_for_rent_exemption(0);
    let i = system_instruction::transfer(&ap(h.key(1)), &ap(sol.vault), reserve + AMOUNT);
    succeeds(&mut h, &[0, 1], i);
    let i = sol.registration(&h);
    succeeds(&mut h, &[0], i);
    assert_eq!(sol.custody(&h), AMOUNT);
    assert_eq!(h.record(sol.root).credit, 0);
    let i = sol.registration(&h);
    succeeds(&mut h, &[0], i);
    assert_eq!(sol.custody(&h), AMOUNT);
    assert_eq!(h.record(sol.root).credit, 0);
    let parent = branch(&mut h, sol.root, 0, true);
    let receiver = h.key(2);
    let i = sol.movement(
        &h,
        (h.key(0), 1),
        h.key(1),
        (parent, receiver),
        AMOUNT,
        true,
    );
    succeeds(&mut h, &[0, 1], i);
    let i = system_instruction::transfer(&ap(h.key(1)), &ap(sol.vault), AMOUNT);
    succeeds(&mut h, &[0, 1], i);
    assert_eq!(sol.custody(&h), 3 * AMOUNT);
    assert_eq!(h.record(sol.root).credit, u128::from(AMOUNT));
    // An empty funding wallet cannot claim the donated SOL.
    let mut empty = h.svm.get_account(&h.key(1)).unwrap();
    empty.lamports = 0;
    h.svm.set_account(h.key(1), empty).unwrap();
    let i = sol.movement(
        &h,
        (h.key(0), 1),
        h.key(1),
        (parent, receiver),
        AMOUNT,
        true,
    );
    rejects(&mut h, &[0, 1], i, 1); // SystemError::ResultWithNegativeLamports.
    let i = sol.movement(
        &h,
        (h.key(0), 0),
        h.key(2),
        (parent, receiver),
        AMOUNT,
        false,
    );
    succeeds(&mut h, &[0], i);
    assert_eq!(sol.custody(&h), 2 * AMOUNT);
    assert_eq!(h.record(sol.root).credit, 0);
}

#[test]
fn native_sol_permissions_admission_and_account_substitutions_reject() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let parent = branch(&mut h, sol.root, 0, true);
    let user = h.key(2);
    let valid = sol.movement(&h, (h.key(0), 1), h.key(1), (parent, user), AMOUNT, true);
    let i = sol.movement(&h, (h.key(0), 0), h.key(1), (parent, user), AMOUNT, true);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let mut i = valid.clone();
    for account in &mut i.accounts {
        if account.pubkey == h.key(1) {
            account.is_signer = false;
        }
    }
    rejects(&mut h, &[0], i, ErrorCode::AccountNotSigner.into());
    let i = sol.movement(&h, (h.key(2), 1), h.key(1), (parent, user), AMOUNT, true);
    rejects(&mut h, &[0, 1, 2], i, LedgerError::Unauthorized.into());
    let i = sol.movement(
        &h,
        (h.key(0), 1),
        h.key(1),
        (sol.root, h.key(0)),
        AMOUNT,
        true,
    );
    rejects(&mut h, &[0, 1], i, LedgerError::InvalidKind.into());
    let i = sol.movement(&h, (h.key(0), 0), sol.vault, (parent, user), 0, false);
    rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
    for target in [sol.root_storage, sol.vault] {
        let mut i = valid.clone();
        for account in &mut i.accounts {
            if account.pubkey == target {
                account.pubkey = h.key(2);
            }
        }
        rejects(&mut h, &[0, 1], i, ErrorCode::ConstraintSeeds.into());
    }
    let original = h.svm.get_account(&sol.vault).unwrap();
    let mut wrong_owner = original.clone();
    wrong_owner.owner = sa(ledger::ID);
    h.svm.set_account(sol.vault, wrong_owner).unwrap();
    rejects(
        &mut h,
        &[0, 1],
        valid.clone(),
        ErrorCode::AccountNotSystemOwned.into(),
    );
    let mut wrong_data = original.clone();
    wrong_data.data = vec![0; 8];
    h.svm.set_account(sol.vault, wrong_data).unwrap();
    rejects(
        &mut h,
        &[0, 1],
        valid.clone(),
        LedgerError::InvalidAccount.into(),
    );
    h.svm.set_account(sol.vault, original).unwrap();
    succeeds(&mut h, &[0, 1], valid);
    let i = sol.movement(&h, (h.key(2), 0), h.key(2), (parent, user), 1, false);
    rejects(&mut h, &[0, 2], i, LedgerError::Unauthorized.into());
    let restricted = branch(&mut h, sol.root, 2, false);
    for deposit in [true, false] {
        let i = sol.movement(&h, (h.key(2), 1), h.key(1), (restricted, user), 0, deposit);
        succeeds(&mut h, &[0, 1, 2], i);
    }
}

#[test]
fn native_sol_full_backing_and_rent_reserve_are_enforced() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let parent = branch(&mut h, sol.root, 0, true);
    let receiver = h.key(2);
    let i = sol.movement(
        &h,
        (h.key(0), 1),
        h.key(1),
        (parent, receiver),
        AMOUNT,
        true,
    );
    succeeds(&mut h, &[0, 1], i);
    let mut vault = h.svm.get_account(&sol.vault).unwrap();
    let reserve = h.svm.minimum_balance_for_rent_exemption(0);
    for balance in [reserve + AMOUNT / 2, reserve - 1] {
        vault.lamports = balance;
        h.svm.set_account(sol.vault, vault.clone()).unwrap();
        for deposit in [false, true] {
            for amount in [0, 1] {
                let i = sol.movement(
                    &h,
                    (h.key(0), 0),
                    h.key(0),
                    (parent, receiver),
                    amount,
                    deposit,
                );
                rejects(&mut h, &[0], i, LedgerError::Undercollateralized.into());
            }
        }
        let fresh = Address::new_from_array([77; 32]);
        for i in [
            transfer(
                &h,
                sol.root,
                h.key(0),
                (parent, receiver),
                (parent, fresh),
                1,
                &[],
            ),
            transfer(
                &h,
                sol.root,
                h.key(0),
                (parent, receiver),
                (parent, receiver),
                0,
                &[],
            ),
            leaf(&h, sol.root, h.key(0), parent, fresh, "New", false),
            group(&h, sol.root, h.key(0), parent, fresh, true, &[]),
            remove(&h, sol.root, h.key(0), parent, fresh, false),
            remove(&h, sol.root, h.key(0), parent, fresh, true),
            sol.registration(&h),
        ] {
            rejects(&mut h, &[0], i, LedgerError::Undercollateralized.into());
        }
        let mut reader = ledger::ledger_view::Reader::new();
        for key in [sol.root_storage, parent, child(parent, receiver)] {
            let account = h.svm.get_account(&key).unwrap();
            reader
                .insert(ap(key), &ap(account.owner), &account.data)
                .unwrap();
        }
        assert_eq!(
            reader.total_supply(&ap(sol.root)).unwrap(),
            u128::from(AMOUNT)
        );
        assert_eq!(
            reader
                .account_view(&ap(sol.root), &ap(parent), &ap(receiver))
                .unwrap()
                .balances
                .debit,
            u128::from(AMOUNT)
        );
    }
    let before = [
        sol.root_storage,
        sol.source,
        parent,
        child(parent, receiver),
    ]
    .map(|key| (key, h.svm.get_account(&key)));
    // Repair with a real System transfer. Rent is restored but never credited.
    let repair = system_instruction::transfer(&ap(h.key(0)), &ap(sol.vault), AMOUNT + 1);
    succeeds(&mut h, &[0], repair);
    for (key, account) in before {
        assert_eq!(h.svm.get_account(&key), account);
    }
    let i = sol.movement(&h, (h.key(0), 0), h.key(2), (parent, receiver), 1, false);
    succeeds(&mut h, &[0], i);
    assert_eq!(sol.custody(&h), AMOUNT - 1);
    assert_eq!(h.record(sol.root).credit, u128::from(AMOUNT - 1));
}

#[test]
fn native_sol_late_commit_rolls_back_deposit_withdrawal_and_allocation() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let parent = branch(&mut h, sol.root, 0, true);
    let user = h.key(2);
    let deposit = sol.movement(&h, (h.key(0), 1), h.key(1), (parent, user), AMOUNT, true);
    let mut readonly = deposit.clone();
    for account in &mut readonly.accounts {
        if account.pubkey == parent {
            account.is_writable = false;
        }
    }
    rejects(
        &mut h,
        &[0, 1],
        readonly,
        LedgerError::InvalidAccount.into(),
    );
    assert!(h.svm.get_account(&child(parent, user)).is_none());
    assert_eq!(sol.custody(&h), 0);
    succeeds(&mut h, &[0, 1], deposit);
    let mut withdrawal = sol.movement(&h, (h.key(0), 0), h.key(2), (parent, user), AMOUNT, false);
    for account in &mut withdrawal.accounts {
        if account.pubkey == parent {
            account.is_writable = false;
        }
    }
    rejects(&mut h, &[0], withdrawal, LedgerError::InvalidAccount.into());
    assert_eq!(sol.custody(&h), AMOUNT);
    assert_eq!(h.record(child(parent, user)).debit, u128::from(AMOUNT));
}

#[test]
fn native_ledger_registration_is_idempotent_without_rent_or_events() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let instruction = h.indexed(sol.registration(&h));
    let before: Vec<_> = instruction
        .accounts
        .iter()
        .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
        .collect();
    let result = run_raw(&mut h, &[0], instruction).unwrap();
    assert!(events::event_bytes(&result.logs).is_empty());
    for (key, old) in before {
        let mut current = h.svm.get_account(&key);
        if key == h.key(0) {
            current.as_mut().unwrap().lamports += result.fee;
        }
        assert_eq!(current, old);
    }
    assert_eq!(
        (
            h.labels(sol.root).symbol.as_str(),
            h.labels(sol.root).decimals
        ),
        ("SOL", 9)
    );
}

#[test]
fn native_sol_lean_siblings_preserve_backing_and_rent_checks() {
    let mut h = Harness::new();
    let sol = Sol::new(&mut h);
    let alice = h.key(0);
    let bob = h.key(1);
    for relative in [alice, bob] {
        let deposit = sol.movement(&h, (relative, 0), alice, (sol.root, relative), 100, true);
        succeeds(
            &mut h,
            if relative == alice { &[0] } else { &[0, 1] },
            deposit,
        );
    }
    let before = [sol.root, sol.vault].map(|key| h.svm.get_account(&key));
    let call = super::packed_transfers::fast(
        &h,
        sol.root,
        alice,
        (sol.root, alice),
        (sol.root, bob),
        7,
        &[sol.vault],
    );
    let result = run_raw(&mut h, &[0], call.clone()).unwrap();
    eprintln!(
        "PACKED_NATIVE_SIBLING_TRANSFER_CU={}",
        result.compute_units_consumed
    );
    assert_eq!(h.record(child(sol.root, alice)).debit, 93);
    assert_eq!(h.record(child(sol.root, bob)).debit, 107);
    assert_eq!(
        [sol.root, sol.vault].map(|key| h.svm.get_account(&key)),
        before
    );
    assert_eq!(events::event_bytes(&result.logs).len(), 2);
    let vault = h.svm.get_account(&sol.vault).unwrap();
    let endpoints =
        [child(sol.root, alice), child(sol.root, bob)].map(|key| h.svm.get_account(&key));
    for lamports in [
        vault.lamports - 1,
        h.svm.minimum_balance_for_rent_exemption(0) - 1,
    ] {
        let mut underbacked = vault.clone();
        underbacked.lamports = lamports;
        h.svm.set_account(sol.vault, underbacked).unwrap();
        assert!(run_raw(&mut h, &[0], call.clone()).is_err());
        assert_eq!(
            [child(sol.root, alice), child(sol.root, bob)].map(|key| h.svm.get_account(&key)),
            endpoints
        );
    }
}
