use super::*;

fn proxy(
    program: Address,
    user: Address,
    controller: Address,
    mut inner: Instruction,
) -> Instruction {
    for meta in &mut inner.accounts {
        if meta.pubkey == controller {
            meta.is_signer = false;
        }
    }
    let mut accounts = vec![
        AccountMeta::new_readonly(user, true),
        AccountMeta::new_readonly(inner.program_id, false),
    ];
    accounts.extend(inner.accounts);
    let mut data = vec![0];
    data.extend(inner.data);
    Instruction {
        program_id: program,
        accounts,
        data,
    }
}

#[test]
fn independent_applications_control_only_their_own_claims_through_cpi() {
    let mut h = Harness::new();
    let a = h.asset();
    let bytes = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_consumer_test.so"),
    )
    .unwrap();
    let apps = [h.address(), h.address()];
    let controllers: [Address; 2] = std::array::from_fn(|i| {
        Address::find_program_address(&[b"controller", h.keys[i].pubkey().as_ref()], &apps[i]).0
    });
    let positions: [Address; 2] =
        std::array::from_fn(|i| pda(&[b"position", a.asset.as_ref(), controllers[i].as_ref()]));
    for i in 0..2 {
        h.svm.add_program(apps[i], &bytes).unwrap();
        h.svm.airdrop(&controllers[i], 1_000_000_000).unwrap();
        let open = ix(
            accounts::OpenPosition {
                owner: ap(controllers[i]),
                asset: ap(a.asset),
                position: ap(positions[i]),
                system_program: ap(SYSTEM),
            },
            instruction::OpenPosition {},
        );
        h.run(
            i,
            &[proxy(apps[i], h.keys[i].pubkey(), controllers[i], open)],
        )
        .unwrap();
    }
    h.run(2, &[h.move_ix(&a, 2, 100, true)]).unwrap();
    let send = |owner: Address, source: Address, destination: Address, amount| {
        ix(
            accounts::TransferClaims {
                owner: ap(owner),
                asset: ap(a.asset),
                source: ap(source),
                destination: ap(destination),
            },
            instruction::TransferClaims { amount },
        )
    };
    h.run(
        2,
        &[send(h.keys[2].pubkey(), a.positions[2], positions[0], 100)],
    )
    .unwrap();
    let before = [a.asset, a.vault, positions[0], positions[1]].map(|key| h.svm.get_account(&key));
    let steal = send(controllers[0], positions[0], positions[1], 100);
    assert!(h
        .run(
            1,
            &[proxy(
                apps[1],
                h.keys[1].pubkey(),
                controllers[0],
                steal.clone()
            )]
        )
        .is_err());
    assert!(h
        .run(
            1,
            &[proxy(
                apps[0],
                h.keys[1].pubkey(),
                controllers[0],
                steal.clone()
            )]
        )
        .is_err());
    let mut raw = steal.clone();
    raw.accounts[0].is_signer = false;
    assert!(h.run(0, &[raw]).is_err());
    assert_eq!(
        [a.asset, a.vault, positions[0], positions[1]].map(|key| h.svm.get_account(&key)),
        before
    );
    h.run(
        0,
        &[proxy(apps[0], h.keys[0].pubkey(), controllers[0], steal)],
    )
    .unwrap();
    assert_eq!(h.state::<Position>(positions[0]).balance, 0);
    assert_eq!(h.state::<Position>(positions[1]).balance, 100);
    assert_eq!(h.svm.get_account(&a.asset), before[0]);
    assert_eq!(h.svm.get_account(&a.vault), before[1]);
}

#[test]
fn separate_creators_can_register_the_same_mint_without_sharing_claims_or_vaults() {
    let mut h = Harness::new();
    let a = h.asset();
    let other = pda(&[b"ledger", h.keys[1].pubkey().as_ref(), &0u64.to_le_bytes()]);
    h.run(
        1,
        &[ix(
            accounts::InitializeLedger {
                authority: ap(h.keys[1].pubkey()),
                ledger: ap(other),
                system_program: ap(SYSTEM),
            },
            instruction::InitializeLedger { id: 0 },
        )],
    )
    .unwrap();
    h.ledger = other;
    let mut b = a.clone();
    b.asset = pda(&[b"asset", other.as_ref(), a.mint.as_ref()]);
    b.vault = pda(&[b"vault", b.asset.as_ref()]);
    b.positions =
        std::array::from_fn(|i| pda(&[b"position", b.asset.as_ref(), h.keys[i].pubkey().as_ref()]));
    assert!(h.run(0, &[h.register_ix(&b, 0)]).is_err());
    h.run(1, &[h.register_ix(&b, 1)]).unwrap();
    for i in 0..3 {
        h.run(i, &[h.open_ix(&b, i)]).unwrap();
    }
    h.run(
        1,
        &[h.move_ix(&a, 1, 100, true), h.move_ix(&b, 1, 200, true)],
    )
    .unwrap();
    assert_eq!(h.token(a.vault).amount, 100);
    assert_eq!(h.token(b.vault).amount, 200);
    let before = h.snapshot(&b);
    let mut mixed = h.transfer_ix(&a, 1, 2, 100);
    mixed.accounts[3].pubkey = b.positions[2];
    assert!(h.run(1, &[mixed]).is_err());
    assert_eq!(h.snapshot(&b), before);
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 100);
    assert_eq!(h.state::<Asset>(b.asset).total_claims, 200);
}
