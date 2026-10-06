//! Reproducible direct-token comparison; setup is outside measured transfers.
use super::*;
use solana_compute_budget_interface::ComputeBudgetInstruction;

fn measure(h: &mut Harness, label: &str, ix: Instruction) -> serde_json::Value {
    let ix = h.indexed(ix);
    h.svm.expire_blockhash();
    let before = h.account(&h.key(0)).unwrap().lamports;
    let tx = Transaction::new_signed_with_payer(
        &[
            ComputeBudgetInstruction::set_compute_unit_limit(200_000),
            ix,
        ],
        Some(&h.key(0)),
        &[&h.keys[0]],
        h.svm.latest_blockhash(),
    );
    let bytes = wincode::serialize(&tx).unwrap().len();
    assert!(bytes <= 1232);
    let keys = tx.message.account_keys.len();
    let writable = (0..keys)
        .filter(|i| {
            tx.message.is_maybe_writable_with_reserved_addresses(
                *i,
                None::<&std::collections::BTreeSet<Address>>,
            )
        })
        .count();
    let result = h.svm.send_transaction(tx).unwrap();
    serde_json::json!({
        "operation":label, "transaction_cu":result.compute_units_consumed,
        "instruction_cu":result.compute_units_consumed - 150,
        "fee_lamports":result.fee, "new_rent_lamports":before-h.account(&h.key(0)).unwrap().lamports-result.fee,
        "transaction_bytes":bytes,"account_keys":keys,"writable_keys":writable,"logs":result.logs,
    })
}

#[test]
fn profile_mapped_transfer_against_external_token_transfer() {
    let mut h = Harness::new();
    let e = External::setup(&mut h, 110, 0, TOKEN);
    h.metadata(e.mint, "Reward", "RWD");
    let mut mint = Mint::unpack(&h.account(&e.mint).unwrap().data).unwrap();
    mint.supply = 1_000_000;
    h.pack(e.mint, mint);
    let mut wallet = TokenAccount::unpack(&h.account(&e.wallet).unwrap().data).unwrap();
    wallet.amount = 1_000_000;
    h.pack(e.wallet, wallet);
    let alice = h.key(0);
    let bob = h.key(1);
    let recipient = Address::new_from_array([112; 32]);
    wallet.owner = bob;
    wallet.amount = 0;
    h.pack(recipient, wallet);
    let registration = e.registration(&h);
    succeeds(&mut h, &[0], registration);
    let external = spl_token_interface::instruction::transfer_checked(
        &TOKEN,
        &e.wallet,
        &e.mint,
        &recipient,
        &alice,
        &[],
        10,
        6,
    )
    .unwrap();
    let mut rows = vec![measure(&mut h, "external_checked_existing", external)];
    assert_eq!(h.token(recipient), 10);
    let parent = h.remember(e.root, alice);
    let group_ix = ix(
        h.base(0, e.root),
        instruction::AddSubAccountGroup {
            parent: ap(e.root),
            relative: ap(alice),
            name: "App".into(),
            credit: false,
            implicit_allowed: true,
        },
        &[parent],
    );
    succeeds(&mut h, &[0], group_ix);
    for (relative, amount) in [(alice, 100_000), (bob, 1)] {
        let deposit = e.movement(&h, (alice, 0), (parent, relative), amount, true, &[]);
        succeeds(&mut h, &[0], deposit);
    }
    let root_before = h.account(&e.root_storage).unwrap();
    let vault_before = h.account(&e.vault).unwrap();
    for (label, to, amount) in [
        ("ledger_existing", bob, 10),
        ("ledger_new_recipient", h.key(2), 10),
        ("ledger_self", alice, 10),
        ("ledger_zero", bob, 0),
    ] {
        let mut call = transfer(
            &h,
            e.root,
            alice,
            (parent, alice),
            (parent, to),
            amount,
            &[],
        );
        for meta in &mut call.accounts {
            if meta.pubkey == e.root_storage
                || ((amount == 0 || to == alice) && meta.pubkey == h.storage(parent))
            {
                meta.is_writable = false;
            }
        }
        rows.push(measure(&mut h, label, call));
    }
    assert_eq!(h.record(child(parent, alice)).debit, 99_980);
    assert_eq!(h.record(child(parent, bob)).debit, 11);
    assert_eq!(h.record(child(parent, h.key(2))).debit, 10);
    assert_eq!(h.account(&e.root_storage).unwrap(), root_before);
    assert_eq!(h.account(&e.vault).unwrap(), vault_before);
    let report = serde_json::to_string_pretty(&rows).unwrap();
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/transfer-costs.json"),
        &report,
    )
    .unwrap();
    eprintln!("{report}");
}
