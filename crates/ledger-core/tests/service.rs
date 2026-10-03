use cavalre_ledger_core::*;

fn id(n: u8) -> Id {
    [n; 32]
}
fn auth(signers: &[Id]) -> Authorization<'_> {
    Authorization::from_verified_signers(signers)
}
fn namespace() -> Namespace {
    initialize_namespace(id(1), id(2), auth(&[id(2)])).unwrap()
}
fn root(kind: RootKind) -> Root {
    create_root(&namespace(), id(3), kind, auth(&[id(2)])).unwrap()
}
fn account(root: &Root, n: u8, controller: u8, kind: Kind) -> Account {
    create_node(
        root,
        &namespace(),
        None,
        NewNode {
            id: id(n),
            controller: id(controller),
            kind,
        },
        auth(&[id(2), id(controller)]),
    )
    .unwrap()
    .account
}
fn post(
    root: &Root,
    records: &[Account],
    amount: u128,
    operation: Operation,
    signers: &[Id],
) -> Result<Posting, Error> {
    plan_posting(
        root,
        records,
        0,
        records.len() - 1,
        amount,
        operation,
        auth(signers),
    )
}

#[test]
fn namespace_creation_and_root_registration_require_the_actual_creator() {
    assert_eq!(
        initialize_namespace(id(1), id(2), auth(&[id(9)])),
        Err(Error::Unauthorized)
    );
    let ns = namespace();
    assert_eq!(
        create_root(&ns, id(3), RootKind::Asset, auth(&[id(9)])),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        create_root(&ns, ns.id, RootKind::Journal, auth(&[id(2)])),
        Err(Error::InvalidNode)
    );
    let root = root(RootKind::Asset);
    assert_eq!(
        open_position(&root, id(4), id(5), auth(&[id(2)])),
        Err(Error::Unauthorized)
    );
    // Permissionless holder positions do not require namespace consent.
    assert!(open_position(&root, id(4), id(5), auth(&[id(5)])).is_ok());
}

#[test]
fn structural_authority_never_confers_spending_authority_even_for_noops() {
    let root = root(RootKind::Journal);
    let source = account(&root, 4, 10, Kind::CreditLeaf);
    let holder = account(&root, 5, 11, Kind::DebitLeaf);
    for amount in [0, 1] {
        for signers in [&[][..], &[id(2)][..], &[id(10)][..], &[id(11)][..]] {
            assert_eq!(
                post(
                    &root,
                    &[source, holder],
                    amount,
                    Operation::PostJournal,
                    signers
                ),
                Err(Error::Unauthorized)
            );
        }
    }
    let plan = post(
        &root,
        &[source, holder],
        7,
        Operation::PostJournal,
        &[id(10), id(11)],
    )
    .unwrap();
    assert_eq!(
        plan.changes()
            .iter()
            .find(|c| c.id == root.id)
            .unwrap()
            .after,
        Balances {
            debit: 7,
            credit: 7
        }
    );
    assert_eq!(
        plan_posting(
            &root,
            &[holder],
            0,
            0,
            0,
            Operation::PostJournal,
            auth(&[id(2)])
        ),
        Err(Error::Unauthorized)
    );
}

#[test]
fn root_types_prevent_unbacked_claim_issuance() {
    let asset = root(RootKind::Asset);
    let holder = account(&asset, 4, 10, Kind::DebitLeaf);
    assert_eq!(
        create_node(
            &asset,
            &namespace(),
            None,
            NewNode {
                id: id(5),
                controller: id(10),
                kind: Kind::CreditLeaf
            },
            auth(&[id(2), id(10)])
        ),
        Err(Error::UnsupportedRoot)
    );
    for operation in [Operation::PostJournal, Operation::TransferJournal] {
        assert_eq!(
            plan_posting(&asset, &[holder], 0, 0, 0, operation, auth(&[id(10)])),
            Err(Error::UnsupportedRoot)
        );
    }
    let journal = Root {
        kind: RootKind::Journal,
        ..asset
    };
    assert_eq!(
        plan_posting(
            &journal,
            &[holder],
            0,
            0,
            0,
            Operation::TransferClaims,
            auth(&[id(10)])
        ),
        Err(Error::UnsupportedRoot)
    );
    assert_eq!(
        prepare_deposit(
            &journal,
            &holder,
            1,
            TokenBalances {
                vault: 0,
                wallet: 10
            },
            auth(&[id(10)])
        ),
        Err(Error::UnsupportedRoot)
    );
}

#[test]
fn public_self_transfers_check_funds_but_internal_journal_noops_do_not() {
    let mut root = root(RootKind::Journal);
    root.gross = 10;
    let mut holder = account(&root, 4, 10, Kind::DebitLeaf);
    holder.node.balances.debit = 10;
    assert_eq!(
        plan_posting(
            &root,
            &[holder],
            0,
            0,
            11,
            Operation::TransferJournal,
            auth(&[id(10)])
        ),
        Err(Error::InsufficientBalance)
    );
    assert!(plan_posting(
        &root,
        &[holder],
        0,
        0,
        11,
        Operation::PostJournal,
        auth(&[id(10)])
    )
    .unwrap()
    .changes()
    .is_empty());
    root.kind = RootKind::Asset;
    assert_eq!(
        plan_posting(
            &root,
            &[holder],
            0,
            0,
            11,
            Operation::TransferClaims,
            auth(&[id(10)])
        ),
        Err(Error::InsufficientBalance)
    );
    assert!(plan_posting(
        &root,
        &[holder],
        0,
        0,
        10,
        Operation::TransferClaims,
        auth(&[id(10)])
    )
    .unwrap()
    .changes()
    .is_empty());
}

fn nested(root: &Root, parent: &Account, n: u8, controller: u8, kind: Kind) -> Creation {
    create_node(
        root,
        &namespace(),
        Some(parent),
        NewNode {
            id: id(n),
            controller: id(controller),
            kind,
        },
        auth(&[parent.controller, id(controller)]),
    )
    .unwrap()
}

#[test]
fn nested_routes_require_complete_unique_paths_and_source_consent() {
    let mut root = root(RootKind::Asset);
    root.gross = 20;
    let mut group = account(&root, 4, 10, Kind::CreditGroup);
    let mut a = nested(&root, &group, 5, 11, Kind::DebitLeaf).account;
    let b = nested(&root, &group, 6, 12, Kind::DebitLeaf).account;
    group.children = 2;
    group.node.balances.debit = 20;
    a.node.balances.debit = 20;
    let records = [a, group, b];
    assert_eq!(
        post(
            &root,
            &records,
            7,
            Operation::TransferClaims,
            &[id(2), id(10)]
        ),
        Err(Error::Unauthorized)
    );
    let plan = post(&root, &records, 7, Operation::TransferClaims, &[id(11)]).unwrap();
    assert_eq!(plan.changes().len(), 2);
    assert_eq!(plan.changes()[0].after.debit, 13);
    assert_eq!(plan.changes()[1].after.debit, 7);
    assert_eq!(plan.from.custody, group.node.id);
    assert_eq!(plan.to.custody, group.node.id);
    assert!(post(&root, &[a, b], 7, Operation::TransferClaims, &[id(11)]).is_err());
    assert!(post(
        &root,
        &[a, group, group, b],
        7,
        Operation::TransferClaims,
        &[id(11)]
    )
    .is_err());
    let extra = account(&root, 7, 13, Kind::DebitLeaf);
    assert!(post(
        &root,
        &[a, group, extra, b],
        7,
        Operation::TransferClaims,
        &[id(11)]
    )
    .is_err());
    assert!(plan_posting(
        &root,
        &records,
        usize::MAX,
        0,
        0,
        Operation::TransferClaims,
        auth(&[id(11)])
    )
    .is_err());
    for bad in [
        Account { depth: 1, ..a },
        Account { root: id(99), ..a },
        Account { children: 1, ..a },
    ] {
        assert!(post(
            &root,
            &[bad, group, b],
            7,
            Operation::TransferClaims,
            &[id(11)]
        )
        .is_err());
    }
    let mut cycle = group;
    cycle.node.parent = Some(a.node.id);
    assert!(post(
        &root,
        &[a, cycle, b],
        7,
        Operation::TransferClaims,
        &[id(11)]
    )
    .is_err());
    assert!(plan_posting(
        &root,
        &[group],
        0,
        0,
        0,
        Operation::TransferClaims,
        auth(&[id(10)])
    )
    .is_err());
    root.kind = RootKind::Journal;
    assert!(post(&root, &records, 7, Operation::TransferJournal, &[id(11)]).is_err());
}

#[test]
fn node_lifecycle_preserves_controller_consent_and_child_counts() {
    let root = root(RootKind::Journal);
    let mut group = account(&root, 4, 10, Kind::DebitGroup);
    let new = NewNode {
        id: id(5),
        controller: id(11),
        kind: Kind::DebitLeaf,
    };
    for signers in [&[id(10)][..], &[id(2), id(11)][..]] {
        assert_eq!(
            create_node(&root, &namespace(), Some(&group), new, auth(signers)),
            Err(Error::Unauthorized)
        );
    }
    let creation = nested(&root, &group, 5, 11, Kind::DebitLeaf);
    assert_eq!(
        creation.parent,
        Some(ChildrenChange {
            id: group.node.id,
            before: 0,
            after: 1
        })
    );
    group.children = 1;
    assert_eq!(
        close_node(&root, &group, None, auth(&[id(10)])),
        Err(Error::NonemptyNode)
    );
    assert_eq!(
        close_node(&root, &creation.account, Some(&group), auth(&[id(10)])),
        Err(Error::Unauthorized)
    );
    assert_eq!(
        close_node(&root, &creation.account, None, auth(&[id(11)])),
        Err(Error::InvalidNode)
    );
    assert_eq!(
        close_node(&root, &creation.account, Some(&group), auth(&[id(11)])).unwrap(),
        Some(ChildrenChange {
            id: group.node.id,
            before: 1,
            after: 0
        })
    );
    group.children = 0;
    group.node.balances = Balances {
        debit: 1,
        credit: 1,
    };
    assert_eq!(
        close_node(&root, &group, None, auth(&[id(10)])),
        Err(Error::NonemptyNode)
    );
    group.node.balances = Balances::default();
    assert_eq!(
        close_node(&root, &group, None, auth(&[id(10)])).unwrap(),
        None
    );
    group.children = u32::MAX;
    assert_eq!(
        create_node(
            &root,
            &namespace(),
            Some(&group),
            new,
            auth(&[id(10), id(11)])
        ),
        Err(Error::Overflow)
    );
}

#[test]
fn hierarchy_bound_is_enforced_at_creation_and_posting() {
    let root = root(RootKind::Journal);
    let mut group = account(&root, 4, 10, Kind::DebitGroup);
    let mut ancestors = vec![group];
    for n in 5..19 {
        group = nested(&root, &group, n, 10, Kind::DebitGroup).account;
        ancestors.push(group);
    }
    assert_eq!(group.depth, 15);
    let leaf = nested(&root, &group, 19, 10, Kind::DebitLeaf).account;
    assert_eq!(leaf.depth, 16);
    ancestors.reverse();
    ancestors.insert(0, leaf);
    assert!(plan_posting(
        &root,
        &ancestors,
        0,
        0,
        0,
        Operation::PostJournal,
        auth(&[id(10)])
    )
    .is_ok());
    let deepest = nested(&root, &group, 20, 10, Kind::DebitGroup).account;
    assert_eq!(
        create_node(
            &root,
            &namespace(),
            Some(&deepest),
            NewNode {
                id: id(21),
                controller: id(10),
                kind: Kind::DebitLeaf
            },
            auth(&[id(10)])
        ),
        Err(Error::DepthLimit)
    );
}

#[test]
fn custody_requires_exact_two_sided_settlement_and_keeps_donations_as_surplus() {
    let mut root = root(RootKind::Asset);
    let mut holder = open_position(&root, id(4), id(10), auth(&[id(10)])).unwrap();
    let before = TokenBalances {
        vault: 50,
        wallet: 100,
    };
    assert_eq!(
        prepare_deposit(&root, &holder, 20, before, auth(&[id(2)])),
        Err(Error::Unauthorized)
    );
    let plan = prepare_deposit(&root, &holder, 20, before, auth(&[id(10)])).unwrap();
    assert_eq!(plan.root().after.debit, 20); // donation 50 issues no claims
    assert_eq!(root.gross, 0);
    assert_eq!(holder.node.balances.debit, 0); // preparation is pure
    for after in [
        TokenBalances {
            vault: 69,
            wallet: 80,
        },
        TokenBalances {
            vault: 70,
            wallet: 79,
        },
        TokenBalances {
            vault: 70,
            wallet: 100,
        },
    ] {
        assert_eq!(
            plan.verify_settlement(after),
            Err(Error::UnexpectedTokenDelta)
        );
    }
    plan.verify_settlement(TokenBalances {
        vault: 70,
        wallet: 80,
    })
    .unwrap();
    root.gross = plan.root().after.debit;
    holder.node.balances = plan.position().after;
    let plan = prepare_withdrawal(
        &root,
        &holder,
        20,
        TokenBalances {
            vault: 70,
            wallet: 80,
        },
        auth(&[id(10)]),
    )
    .unwrap();
    assert_eq!(plan.root().after.debit, 0);
    assert_eq!(plan.root().after.credit, 0);
    assert_eq!(
        plan.verify_settlement(TokenBalances {
            vault: 50,
            wallet: 99
        }),
        Err(Error::UnexpectedTokenDelta)
    );
    plan.verify_settlement(TokenBalances {
        vault: 50,
        wallet: 100,
    })
    .unwrap();
}

#[test]
fn custody_rejects_first_mover_exits_nested_redemption_and_overflow() {
    let mut root = root(RootKind::Asset);
    root.gross = 100;
    let mut holder = account(&root, 4, 10, Kind::DebitLeaf);
    holder.node.balances.debit = 10;
    assert_eq!(
        prepare_withdrawal(
            &root,
            &holder,
            1,
            TokenBalances {
                vault: 99,
                wallet: 0
            },
            auth(&[id(10)])
        ),
        Err(Error::Undercollateralized)
    );
    assert_eq!(
        prepare_withdrawal(
            &root,
            &holder,
            11,
            TokenBalances {
                vault: 100,
                wallet: 0
            },
            auth(&[id(10)])
        ),
        Err(Error::InsufficientBalance)
    );
    let group = account(&root, 5, 10, Kind::DebitGroup);
    let nested = nested(&root, &group, 6, 10, Kind::DebitLeaf).account;
    assert_eq!(
        prepare_deposit(
            &root,
            &nested,
            1,
            TokenBalances {
                vault: 100,
                wallet: 10
            },
            auth(&[id(10)])
        ),
        Err(Error::InvalidNode)
    );
    root.gross = u128::MAX;
    assert_eq!(
        prepare_deposit(
            &root,
            &holder,
            1,
            TokenBalances {
                vault: 0,
                wallet: 10
            },
            auth(&[id(10)])
        ),
        Err(Error::Overflow)
    );
}

#[test]
fn shared_ancestors_cancel_before_arithmetic_at_u128_limit() {
    let mut root = root(RootKind::Asset);
    root.gross = u128::MAX;
    let mut a = account(&root, 4, 10, Kind::DebitLeaf);
    a.node.balances.debit = u128::MAX;
    let b = account(&root, 5, 11, Kind::DebitLeaf);
    let plan = post(&root, &[a, b], 1, Operation::TransferClaims, &[id(10)]).unwrap();
    assert_eq!(plan.changes().len(), 2);
    assert_eq!(plan.changes()[0].after.debit, u128::MAX - 1);
    assert_eq!(plan.changes()[1].after.debit, 1);
}
