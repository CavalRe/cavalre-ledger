use super::*;
use crate::ledger::{Journal, LedgerNode, NodeKind};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Journal,
    Public,
    Custody,
}

impl Harness {
    fn multi(&mut self, signers: &[usize], instructions: &[Instruction]) -> TransactionResult {
        self.svm.expire_blockhash();
        let mut unique = Vec::new();
        for &i in signers {
            if !unique.contains(&i) {
                unique.push(i);
            }
        }
        let signers: Vec<_> = unique.iter().map(|i| &self.keys[*i]).collect();
        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&signers[0].pubkey()),
            &signers,
            self.svm.latest_blockhash(),
        );
        assert!(
            wincode::serialize(&tx).unwrap().len() <= PACKET_BYTES,
            "test attempted an oversized legacy transaction"
        );
        self.svm.send_transaction(tx).map_err(Box::new)
    }
    fn journal(&mut self) -> Address {
        let id = u64::from(self.next_key);
        self.next_key += 1;
        let journal = pda(&[b"journal", self.ledger.as_ref(), &id.to_le_bytes()]);
        self.run(
            0,
            &[ix(
                accounts::InitializeJournal {
                    authority: ap(self.keys[0].pubkey()),
                    ledger: ap(self.ledger),
                    journal: ap(journal),
                    system_program: ap(SYSTEM),
                },
                instruction::InitializeJournal { id },
            )],
        )
        .unwrap();
        journal
    }
    fn create_node_ix(
        &self,
        root: Address,
        parent: Address,
        relative: Address,
        kind: NodeKind,
        controller: usize,
        parent_authority: usize,
    ) -> (Address, Instruction) {
        let node = pda(&[b"node", root.as_ref(), parent.as_ref(), relative.as_ref()]);
        let mut instruction = ix(
            accounts::CreateNode {
                payer: ap(self.keys[0].pubkey()),
                controller: ap(self.keys[controller].pubkey()),
                parent_authority: ap(self.keys[parent_authority].pubkey()),
                ledger: ap(self.ledger),
                root: ap(root),
                parent: ap(parent),
                node: ap(node),
                system_program: ap(SYSTEM),
            },
            instruction::CreateNode {
                relative: ap(relative),
                kind,
            },
        );
        instruction.accounts[5].is_writable = parent != root;
        (node, instruction)
    }
    fn node(
        &mut self,
        root: Address,
        parent: Address,
        kind: NodeKind,
        controller: usize,
    ) -> Address {
        let relative = self.address();
        let authority = if parent == root {
            0
        } else {
            self.key_index(self.state::<LedgerNode>(parent).controller)
        };
        let (key, instruction) =
            self.create_node_ix(root, parent, relative, kind, controller, authority);
        self.multi(&[0, controller, authority], &[instruction])
            .unwrap();
        key
    }
    fn key_index(&self, key: anchor_lang::prelude::Pubkey) -> usize {
        self.keys
            .iter()
            .position(|k| k.pubkey() == sa(key))
            .unwrap()
    }
    fn node_path(&self, root: Address, leaf: Address) -> Vec<Address> {
        let mut path = vec![];
        let mut key = leaf;
        while key != root {
            assert!(path.len() < 17);
            path.push(key);
            let data = self.svm.get_account(&key).unwrap().data;
            key = if let Ok(n) = LedgerNode::try_deserialize(&mut data.as_slice()) {
                sa(n.parent)
            } else {
                sa(self.state::<Position>(key).asset)
            };
        }
        path
    }
    fn node_controller(&self, key: Address) -> anchor_lang::prelude::Pubkey {
        let data = self.svm.get_account(&key).unwrap().data;
        if let Ok(n) = LedgerNode::try_deserialize(&mut data.as_slice()) {
            n.controller
        } else {
            self.state::<Position>(key).owner
        }
    }
    fn credit(&self, key: Address) -> bool {
        let data = self.svm.get_account(&key).unwrap().data;
        LedgerNode::try_deserialize(&mut data.as_slice())
            .is_ok_and(|n| matches!(n.kind, NodeKind::CreditLeaf | NodeKind::CreditGroup))
    }
    fn posting(
        &self,
        root: Address,
        from: Address,
        to: Address,
        amount: u128,
        mode: Mode,
    ) -> Instruction {
        let a = self.node_path(root, from);
        let b = self.node_path(root, to);
        let mut keys = a.clone();
        for k in &b {
            if !keys.contains(k) {
                keys.push(*k);
            }
        }
        let from_index = keys.iter().position(|k| *k == from).unwrap() as u8;
        let to_index = keys.iter().position(|k| *k == to).unwrap() as u8;
        let mut instruction = match mode {
            Mode::Journal => ix(
                accounts::PostJournal {
                    from_authority: self.node_controller(from),
                    to_authority: self.node_controller(to),
                    root: ap(root),
                },
                instruction::PostJournal {
                    from: from_index,
                    to: to_index,
                    amount,
                },
            ),
            Mode::Public => ix(
                accounts::TransferHierarchy {
                    owner: self.node_controller(from),
                    root: ap(root),
                },
                instruction::TransferJournal {
                    from: from_index,
                    to: to_index,
                    amount,
                },
            ),
            Mode::Custody => ix(
                accounts::TransferHierarchy {
                    owner: self.node_controller(from),
                    root: ap(root),
                },
                instruction::TransferNested {
                    from: from_index,
                    to: to_index,
                    amount: amount.try_into().unwrap(),
                },
            ),
        };
        let same_side = self.credit(from) == self.credit(to);
        instruction.accounts.last_mut().unwrap().is_writable =
            mode == Mode::Journal && !same_side && from != to && amount != 0;
        for key in keys {
            // A shared same-side ancestor cancels. Everything else in the union
            // changes for a nonzero, nonself posting (root handled separately).
            let writable =
                from != to && amount != 0 && !(same_side && a.contains(&key) && b.contains(&key));
            instruction.accounts.push(AccountMeta {
                pubkey: key,
                is_signer: false,
                is_writable: writable,
            });
        }
        instruction
    }
    fn close_node_ix(&self, root: Address, key: Address) -> Instruction {
        let node = self.state::<LedgerNode>(key);
        let mut instruction = ix(
            accounts::CloseNode {
                controller: node.controller,
                rent_payer: node.rent_payer,
                root: ap(root),
                parent: node.parent,
                node: ap(key),
            },
            instruction::CloseNode {},
        );
        instruction.accounts[3].is_writable = sa(node.parent) != root;
        instruction
    }
    fn snap_keys(&self, keys: &[Address]) -> Vec<Option<Account>> {
        keys.iter().map(|k| self.svm.get_account(k)).collect()
    }
}

fn writes(instruction: &Instruction) -> BTreeSet<Address> {
    instruction
        .accounts
        .iter()
        .filter(|a| a.is_writable)
        .map(|a| a.pubkey)
        .collect()
}

#[test]
fn replays_every_solidity_hierarchy_action_in_actual_sbpf() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../spec/fixtures/hierarchy.json")).unwrap();
    let pin: serde_json::Value =
        serde_json::from_str(include_str!("../../spec/upstream.json")).unwrap();
    assert_eq!(fixture["contracts_commit"], pin["references"][0]["commit"]);
    let mut h = Harness::new();
    let root = h.journal();
    let mut keys = vec![root];
    let nodes = fixture["nodes"].as_array().unwrap();
    for (i, n) in nodes.iter().enumerate().skip(1) {
        let kind = match (
            n["credit"].as_bool().unwrap(),
            n["group"].as_bool().unwrap(),
        ) {
            (false, false) => NodeKind::DebitLeaf,
            (true, false) => NodeKind::CreditLeaf,
            (false, true) => NodeKind::DebitGroup,
            (true, true) => NodeKind::CreditGroup,
        };
        let controller = if i == 1 || n["group"] == true {
            0
        } else if i == 3 || n["credit"] == true {
            2
        } else {
            1
        };
        let key = h.node(
            root,
            keys[n["parent"].as_u64().unwrap() as usize],
            kind,
            controller,
        );
        keys.push(key);
    }
    let mut rejected = 0;
    for (i, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let from = keys[step["from"].as_u64().unwrap() as usize];
        let to = keys[step["to"].as_u64().unwrap() as usize];
        let mode = if step["public"] == true {
            Mode::Public
        } else {
            Mode::Journal
        };
        let instruction = h.posting(
            root,
            from,
            to,
            step["amount"].as_u64().unwrap().into(),
            mode,
        );
        let before = h.snap_keys(&keys);
        let mut signers = vec![0, h.key_index(h.node_controller(from))];
        if mode == Mode::Journal {
            signers.push(h.key_index(h.node_controller(to)));
        }
        let result = h.multi(&signers, &[instruction]);
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {i}: {result:?}"
        );
        if result.is_err() {
            rejected += 1;
            assert_eq!(h.snap_keys(&keys), before, "rollback step {i}");
        }
        let mut balances = vec![(
            h.state::<Journal>(root).gross,
            h.state::<Journal>(root).gross,
        )];
        for &key in &keys[1..] {
            let n = h.state::<LedgerNode>(key);
            balances.push((n.debit, n.credit));
        }
        for (j, &(debit, credit)) in balances.iter().enumerate() {
            assert_eq!(
                debit,
                step["balances"][2 * j].as_u64().unwrap() as u128,
                "step {i}, node {j} debit"
            );
            assert_eq!(
                credit,
                step["balances"][2 * j + 1].as_u64().unwrap() as u128,
                "step {i}, node {j} credit"
            );
            if nodes[j]["group"] == true {
                let children =
                    (1..keys.len()).filter(|k| nodes[*k]["parent"].as_u64().unwrap() as usize == j);
                let sums =
                    children.fold((0, 0), |(d, c), k| (d + balances[k].0, c + balances[k].1));
                assert_eq!(
                    (debit, credit),
                    sums,
                    "independent immediate-child sum, step {i}, group {j}"
                );
            }
        }
    }
    assert_eq!(rejected, 17);
}

#[test]
fn custody_nested_routes_are_backed_atomic_and_holder_authorized() {
    let mut h = Harness::new();
    let a = h.asset();
    let group = h.node(a.asset, a.asset, NodeKind::DebitGroup, 0);
    let inner = h.node(a.asset, group, NodeKind::DebitGroup, 0);
    let alice = h.node(a.asset, inner, NodeKind::DebitLeaf, 1);
    let bob = h.node(a.asset, group, NodeKind::DebitLeaf, 2);
    let keys = [
        a.asset,
        a.vault,
        a.wallets[1],
        a.wallets[2],
        a.positions[1],
        a.positions[2],
        group,
        inner,
        alice,
        bob,
    ];
    h.multi(
        &[0, 1],
        &[
            h.move_ix(&a, 1, 100, true),
            h.posting(a.asset, a.positions[1], alice, 100, Mode::Custody),
        ],
    )
    .unwrap();
    assert_eq!(h.state::<Position>(a.positions[1]).balance, 0);
    assert_eq!(h.state::<LedgerNode>(group).debit, 100);
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 100);
    assert_eq!(h.token(a.vault).amount, 100);
    let before = h.snap_keys(&keys);
    let mut theft = h.posting(a.asset, alice, bob, 1, Mode::Custody);
    theft.accounts[0].pubkey = h.keys[0].pubkey();
    custom_error(h.run(0, &[theft]), CustodyError::NodeAuthority);
    assert_eq!(h.snap_keys(&keys), before);
    custom_error(
        h.run(0, &[h.posting(a.asset, group, bob, 1, Mode::Custody)]),
        CustodyError::InvalidPosting,
    );
    custom_error(
        h.multi(
            &[0, 1, 2],
            &[h.posting(a.asset, alice, bob, 1, Mode::Journal)],
        ),
        CustodyError::UnsupportedRoot,
    );
    // A late nested failure must undo the preceding successful SPL CPI too.
    assert!(h
        .multi(
            &[0, 1],
            &[
                h.move_ix(&a, 1, 5, true),
                h.posting(a.asset, a.positions[1], alice, 6, Mode::Custody)
            ]
        )
        .is_err());
    assert_eq!(h.snap_keys(&keys), before);
    h.multi(
        &[0, 1],
        &[h.posting(a.asset, alice, bob, 40, Mode::Custody)],
    )
    .unwrap();
    h.multi(
        &[0, 2],
        &[
            h.posting(a.asset, bob, a.positions[2], 40, Mode::Custody),
            h.move_ix(&a, 2, 40, false),
        ],
    )
    .unwrap();
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 60);
    assert_eq!(h.token(a.vault).amount, 60);
    assert_eq!(h.state::<LedgerNode>(alice).debit, 60);
    assert_eq!(h.state::<LedgerNode>(group).debit, 60);
    assert_eq!(h.state::<LedgerNode>(inner).debit, 60);
    // Failure in the later CPI restores the nested records as well.
    let mut wallet = h.token(a.wallets[1]);
    wallet.state = AccountState::Frozen;
    h.put_packed(a.wallets[1], wallet);
    let before = h.snap_keys(&keys);
    assert!(h
        .multi(
            &[0, 1],
            &[
                h.posting(a.asset, alice, a.positions[1], 60, Mode::Custody),
                h.move_ix(&a, 1, 60, false)
            ]
        )
        .is_err());
    assert_eq!(h.snap_keys(&keys), before);
}

#[test]
fn node_creation_and_closure_preserve_ownership_topology_and_rent() {
    let mut h = Harness::new();
    let root = h.journal();
    let group = h.node(root, root, NodeKind::DebitGroup, 1);
    let leaf = h.node(root, group, NodeKind::DebitLeaf, 2);
    assert_eq!(h.state::<LedgerNode>(group).children, 1);
    custom_error(
        h.multi(&[0, 1], &[h.close_node_ix(root, group)]),
        CustodyError::NonemptyNode,
    );
    let relative = h.address();
    let (_, bad) = h.create_node_ix(root, group, relative, NodeKind::DebitLeaf, 2, 0);
    custom_error(h.multi(&[0, 2], &[bad]), CustodyError::NodeAuthority);
    let (_, bad) = h.create_node_ix(root, leaf, relative, NodeKind::DebitLeaf, 2, 2);
    custom_error(h.multi(&[0, 2], &[bad]), CustodyError::InvalidNode);
    let old = h.state::<LedgerNode>(leaf);
    let (_, flip) = h.create_node_ix(root, group, sa(old.relative), NodeKind::CreditGroup, 2, 1);
    assert!(h.multi(&[0, 1, 2], &[flip]).is_err());
    let source = h.node(root, root, NodeKind::CreditLeaf, 0);
    h.multi(&[0, 2], &[h.posting(root, source, leaf, 1, Mode::Journal)])
        .unwrap();
    custom_error(
        h.multi(&[0, 2], &[h.close_node_ix(root, leaf)]),
        CustodyError::NonemptyNode,
    );
    h.multi(&[0, 2], &[h.posting(root, leaf, source, 1, Mode::Journal)])
        .unwrap();
    let mut redirect = h.close_node_ix(root, leaf);
    redirect.accounts[1].pubkey = h.keys[1].pubkey();
    assert!(h.multi(&[0, 2], &[redirect]).is_err());
    let rent = h.svm.get_account(&leaf).unwrap().lamports;
    let before = h.svm.get_balance(&h.keys[0].pubkey()).unwrap();
    h.run(2, &[h.close_node_ix(root, leaf)]).unwrap();
    assert_eq!(
        h.svm.get_balance(&h.keys[0].pubkey()).unwrap(),
        before + rent
    );
    assert!(h.svm.get_account(&leaf).is_none_or(|a| a.lamports == 0));
    assert_eq!(h.state::<LedgerNode>(group).children, 0);
    h.run(1, &[h.close_node_ix(root, group)]).unwrap();
    let a = h.asset();
    let relative = h.address();
    let (_, bad) = h.create_node_ix(a.asset, a.asset, relative, NodeKind::CreditLeaf, 1, 0);
    custom_error(h.multi(&[0, 1], &[bad]), CustodyError::UnsupportedRoot);
}

#[test]
fn rejects_incomplete_duplicate_extra_substituted_and_readonly_paths_atomically() {
    let mut h = Harness::new();
    let root = h.journal();
    let a = h.node(root, root, NodeKind::DebitGroup, 0);
    let b = h.node(root, root, NodeKind::CreditGroup, 0);
    let source = h.node(root, a, NodeKind::CreditLeaf, 1);
    let dest = h.node(root, b, NodeKind::DebitLeaf, 2);
    let other = h.node(root, root, NodeKind::DebitGroup, 0);
    let good = h.posting(root, source, dest, 7, Mode::Journal);
    let keys = [root, a, b, source, dest, other];
    let before = h.snap_keys(&keys);
    let mut cases = vec![];
    let mut x = good.clone();
    x.accounts.retain(|m| m.pubkey != a);
    cases.push(x);
    let mut x = good.clone();
    x.accounts.push(x.accounts[3].clone());
    cases.push(x);
    let mut x = good.clone();
    x.accounts.push(AccountMeta::new_readonly(other, false));
    cases.push(x);
    let mut x = good.clone();
    x.accounts
        .iter_mut()
        .find(|m| m.pubkey == a)
        .unwrap()
        .pubkey = other;
    cases.push(x);
    for key in [root, a, b, source, dest] {
        let mut x = good.clone();
        x.accounts
            .iter_mut()
            .find(|m| m.pubkey == key)
            .unwrap()
            .is_writable = false;
        cases.push(x);
    }
    let mut x = good.clone();
    x.data = instruction::PostJournal {
        from: 255,
        to: 0,
        amount: 0,
    }
    .data();
    cases.push(x);
    let mut x = good.clone();
    x.accounts.push(AccountMeta::new_readonly(root, false));
    cases.push(x);
    let mut x = good.clone();
    x.accounts[0].pubkey = h.keys[0].pubkey();
    cases.push(x);
    let mut x = good.clone();
    x.accounts[1].pubkey = h.keys[0].pubkey();
    cases.push(x);
    for (i, x) in cases.into_iter().enumerate() {
        // Sign only the keys actually requested by each malicious message.
        let signers: Vec<_> = (0..3)
            .filter(|j| {
                *j == 0
                    || x.accounts
                        .iter()
                        .any(|m| m.pubkey == h.keys[*j].pubkey() && m.is_signer)
            })
            .collect();
        assert!(h.multi(&signers, &[x]).is_err(), "malformed case {i}");
        assert_eq!(h.snap_keys(&keys), before, "rollback case {i}");
    }
    // Unordered input is supported; indexes identify endpoints, parentage is stored.
    let mut reordered = good;
    reordered.accounts[3..].reverse();
    let from = reordered.accounts[3..]
        .iter()
        .position(|m| m.pubkey == source)
        .unwrap() as u8;
    let to = reordered.accounts[3..]
        .iter()
        .position(|m| m.pubkey == dest)
        .unwrap() as u8;
    reordered.data = instruction::PostJournal {
        from,
        to,
        amount: 7,
    }
    .data();
    h.multi(&[0, 1, 2], &[reordered]).unwrap();
    assert_eq!(h.state::<Journal>(root).gross, 7);
}

#[test]
fn authenticates_dynamic_owner_type_pda_root_depth_and_version() {
    let mut h = Harness::new();
    let root = h.journal();
    let group = h.node(root, root, NodeKind::DebitGroup, 0);
    let source = h.node(root, root, NodeKind::CreditLeaf, 0);
    let leaf = h.node(root, group, NodeKind::DebitLeaf, 1);
    let instruction = h.posting(root, source, leaf, 5, Mode::Journal);
    let pristine = h.svm.get_account(&leaf).unwrap();
    for which in 0..9 {
        h.svm.set_account(leaf, pristine.clone()).unwrap();
        let mut raw = pristine.clone();
        match which {
            0 => raw.owner = SYSTEM,
            1 => raw.data[0] ^= 1,
            2 => {
                raw.data.pop();
            }
            3 => raw.data.push(0),
            _ => {
                let mut n = h.state::<LedgerNode>(leaf);
                match which {
                    4 => n.version += 1,
                    5 => n.relative = ap(h.keys[2].pubkey()),
                    6 => n.root = ap(h.ledger),
                    7 => n.depth = 1,
                    8 => n.parent = ap(source),
                    _ => unreachable!(),
                }
                n.try_serialize(&mut raw.data.as_mut_slice()).unwrap();
            }
        }
        h.svm.set_account(leaf, raw).unwrap();
        let before = h.snap_keys(&[root, group, source, leaf]);
        assert!(
            h.multi(&[0, 1], std::slice::from_ref(&instruction))
                .is_err(),
            "fault {which}"
        );
        assert_eq!(h.snap_keys(&[root, group, source, leaf]), before);
    }
    h.svm.set_account(leaf, pristine).unwrap();
    let other = h.journal();
    let mut x = instruction.clone();
    x.accounts[2].pubkey = other;
    assert!(h.multi(&[0, 1], &[x]).is_err());
    let mut x = instruction;
    x.accounts[1].is_signer = false;
    assert!(h.run(0, &[x]).is_err());
}

#[test]
fn shared_ancestors_are_readonly_and_internal_gross_balances_use_u128() {
    let mut h = Harness::new();
    let root = h.journal();
    let group = h.node(root, root, NodeKind::CreditGroup, 0);
    let source = h.node(root, root, NodeKind::CreditLeaf, 0);
    let a = h.node(root, group, NodeKind::DebitLeaf, 1);
    let b = h.node(root, group, NodeKind::DebitLeaf, 2);
    h.multi(
        &[0, 1],
        &[h.posting(root, source, a, u128::MAX, Mode::Journal)],
    )
    .unwrap();
    let before = h.snap_keys(&[root, group, source, a, b]);
    custom_error(
        h.multi(&[0, 1], &[h.posting(root, source, a, 1, Mode::Journal)]),
        CustodyError::ArithmeticOverflow,
    );
    assert_eq!(h.snap_keys(&[root, group, source, a, b]), before);
    let transfer = h.posting(root, a, b, 7, Mode::Journal);
    assert_eq!(writes(&transfer), BTreeSet::from([a, b]));
    h.multi(&[0, 1, 2], &[transfer]).unwrap();
    assert_eq!(h.state::<Journal>(root).gross, u128::MAX);
    assert_eq!(h.state::<LedgerNode>(group).debit, u128::MAX);
    assert_eq!(h.state::<LedgerNode>(a).debit, u128::MAX - 7);
    assert_eq!(h.state::<LedgerNode>(b).debit, 7);
    h.multi(&[0, 2], &[h.posting(root, b, b, u128::MAX, Mode::Journal)])
        .unwrap();
    custom_error(
        h.multi(&[0, 2], &[h.posting(root, b, b, 0, Mode::Public)]),
        CustodyError::InvalidPosting,
    );
    let direct = h.node(root, root, NodeKind::DebitLeaf, 1);
    custom_error(
        h.multi(&[0, 1], &[h.posting(root, direct, direct, 1, Mode::Public)]),
        CustodyError::InsufficientBalance,
    );
    // Group normal balance may be negative. Posting must preserve gross sums,
    // not impose a new group solvency rule absent from LedgerLib.
    let c = h.node(root, group, NodeKind::CreditLeaf, 0);
    h.multi(
        &[0, 1],
        &[h.posting(root, a, source, u128::MAX - 7, Mode::Journal)],
    )
    .unwrap();
    h.multi(&[0, 1], &[h.posting(root, c, direct, 10, Mode::Journal)])
        .unwrap();
    let state = h.state::<LedgerNode>(group);
    assert_eq!((state.debit, state.credit), (7, 10));
}

fn table(h: &mut Harness, addresses: Vec<Address>) -> solana_message::AddressLookupTableAccount {
    use solana_address_lookup_table_interface::instruction::{
        create_lookup_table, extend_lookup_table,
    };
    let payer = h.keys[0].pubkey();
    let slot = h.svm.get_sysvar::<solana_clock::Clock>().slot;
    let (create, key) = create_lookup_table(payer, payer, slot);
    h.run(0, &[create]).unwrap();
    for chunk in addresses.chunks(16) {
        h.run(
            0,
            &[extend_lookup_table(key, payer, Some(payer), chunk.to_vec())],
        )
        .unwrap();
    }
    h.svm.warp_to_slot(slot + 1);
    solana_message::AddressLookupTableAccount { key, addresses }
}

#[test]
fn measure_nested_paths_in_legacy_v0_and_v1_transactions() {
    use solana_message::{v0, v1, VersionedMessage};
    use solana_transaction::versioned::VersionedTransaction;
    let mut rows = vec![];
    for depth in [1, 2, 4, 8, 12, 16] {
        for shape in ["opposite_disjoint", "same_disjoint", "same_shared"] {
            let mut h = Harness::new();
            let root = h.journal();
            let mut left = root;
            let mut right = root;
            for i in 1..depth {
                left = h.node(
                    root,
                    left,
                    if i % 2 == 0 {
                        NodeKind::CreditGroup
                    } else {
                        NodeKind::DebitGroup
                    },
                    0,
                );
                right = if shape == "same_shared" {
                    left
                } else {
                    h.node(root, right, NodeKind::DebitGroup, 0)
                };
            }
            let from = h.node(
                root,
                left,
                if shape == "opposite_disjoint" {
                    NodeKind::CreditLeaf
                } else {
                    NodeKind::DebitLeaf
                },
                1,
            );
            let to = h.node(root, right, NodeKind::DebitLeaf, 2);
            if shape != "opposite_disjoint" {
                let source = h.node(root, root, NodeKind::CreditLeaf, 0);
                h.multi(
                    &[0, 1],
                    &[h.posting(root, source, from, 100, Mode::Journal)],
                )
                .unwrap();
            }
            let post = h.posting(root, from, to, 1, Mode::Journal);
            let lookup = table(
                &mut h,
                post.accounts
                    .iter()
                    .filter(|m| !m.is_signer)
                    .map(|m| m.pubkey)
                    .collect(),
            );
            for format in ["legacy", "v0", "v1"] {
                h.svm.expire_blockhash();
                let instructions = [
                    ComputeBudgetInstruction::set_compute_unit_limit(200_000),
                    post.clone(),
                ];
                let payer = h.keys[0].pubkey();
                let signers = [&h.keys[0], &h.keys[1], &h.keys[2]];
                let message = match format {
                    "legacy" => {
                        VersionedMessage::Legacy(solana_message::Message::new_with_blockhash(
                            &instructions,
                            Some(&payer),
                            &h.svm.latest_blockhash(),
                        ))
                    }
                    "v0" => VersionedMessage::V0(
                        v0::Message::try_compile(
                            &payer,
                            &instructions,
                            std::slice::from_ref(&lookup),
                            h.svm.latest_blockhash(),
                        )
                        .unwrap(),
                    ),
                    "v1" => VersionedMessage::V1(
                        v1::Message::try_compile_with_config(
                            &payer,
                            std::slice::from_ref(&post),
                            h.svm.latest_blockhash(),
                            v1::TransactionConfig::empty()
                                .with_compute_unit_limit(200_000)
                                .with_loaded_accounts_data_size_limit(64 * 1024 * 1024),
                        )
                        .unwrap(),
                    ),
                    _ => unreachable!(),
                };
                let tx = VersionedTransaction::try_new(message, &signers).unwrap();
                let bytes = wincode::serialize(&tx).unwrap().len();
                let limit = if format == "v1" { 4096 } else { PACKET_BYTES };
                let keys: BTreeSet<_> =
                    post.accounts
                        .iter()
                        .map(|m| m.pubkey)
                        .chain([payer, sa(cavalre::ID)])
                        .chain((format != "v1").then_some(
                            ComputeBudgetInstruction::set_compute_unit_limit(1).program_id,
                        ))
                        .collect();
                let data_bytes: usize = keys
                    .iter()
                    .filter_map(|k| h.svm.get_account(k))
                    .map(|a| a.data.len())
                    .sum();
                let before = h.snap_keys(&[root, from, to]);
                let compute = if bytes <= limit {
                    let result = h.svm.send_transaction(tx);
                    Some(
                        result
                            .unwrap_or_else(|e| {
                                panic!("depth {depth}, shape {shape}, format {format}: {e:?}")
                            })
                            .compute_units_consumed,
                    )
                } else {
                    assert_eq!(h.snap_keys(&[root, from, to]), before);
                    None
                };
                rows.push(serde_json::json!({"depth_edges":depth,"shape":shape,"format":format,"transaction_bytes":bytes,"packet_limit":limit,"fits_packet":bytes<=limit,"account_keys":keys.len(),"protocol_writes":writes(&post).len(),"writable_accounts_with_payer":writes(&post).len()+1,"account_data_bytes":data_bytes,"lookup_table_bytes":if format=="v0"{h.svm.get_account(&lookup.key).unwrap().data.len()}else{0},"compute_units":compute}));
            }
        }
    }
    let report = serde_json::to_string_pretty(&rows).unwrap();
    println!("NESTED_PATH_COSTS\n{report}");
    if let Some(path) = std::env::var_os(HIERARCHY_REPORT_ENV) {
        std::fs::write(path, format!("{report}\n")).unwrap();
    }
}

#[test]
fn enforces_depth_bound_and_disjoint_branch_write_sets() {
    let mut h = Harness::new();
    let root = h.journal();
    let mut parent = root;
    for _ in 1..=16 {
        parent = h.node(root, parent, NodeKind::DebitGroup, 0);
    }
    let relative = h.address();
    let (_, too_deep) = h.create_node_ix(root, parent, relative, NodeKind::DebitLeaf, 0, 0);
    custom_error(h.run(0, &[too_deep]), CustodyError::DepthLimit);
    assert_eq!(h.state::<LedgerNode>(parent).children, 0);
    let a = h.asset();
    let g1 = h.node(a.asset, a.asset, NodeKind::DebitGroup, 0);
    let g2 = h.node(a.asset, a.asset, NodeKind::DebitGroup, 0);
    let x = h.node(a.asset, g1, NodeKind::DebitLeaf, 1);
    let y = h.node(a.asset, g1, NodeKind::DebitLeaf, 2);
    let z = h.node(a.asset, g2, NodeKind::DebitLeaf, 1);
    let w = h.node(a.asset, g2, NodeKind::DebitLeaf, 2);
    let left = h.posting(a.asset, x, y, 1, Mode::Custody);
    let right = h.posting(a.asset, z, w, 1, Mode::Custody);
    assert!(writes(&left).is_disjoint(&writes(&right)));
    // A matching numeric holder identity remains a separately seeded direct
    // Position. It cannot substitute for the descendant that shares its bytes.
    let direct = pda(&[b"position", a.asset.as_ref(), x.as_ref()]);
    assert_ne!(x, direct);
    h.multi(
        &[0, 1],
        &[
            h.move_ix(&a, 1, 1, true),
            h.posting(a.asset, a.positions[1], x, 1, Mode::Custody),
        ],
    )
    .unwrap();
    custom_error(
        h.run(1, &[h.posting(a.asset, x, x, 2, Mode::Custody)]),
        CustodyError::InsufficientBalance,
    );
    h.run(1, &[h.posting(a.asset, x, x, 1, Mode::Custody)])
        .unwrap();
    let mut wrong_owner = h.posting(a.asset, x, x, 0, Mode::Custody);
    wrong_owner.accounts[0].pubkey = h.keys[0].pubkey();
    custom_error(h.run(0, &[wrong_owner]), CustodyError::NodeAuthority);
}

#[test]
fn nested_custody_validates_direct_position_identity_and_root_even_for_zero() {
    let mut h = Harness::new();
    let a = h.asset();
    let b = h.asset();
    let group = h.node(a.asset, a.asset, NodeKind::DebitGroup, 0);
    let leaf = h.node(a.asset, group, NodeKind::DebitLeaf, 1);
    let good = h.posting(a.asset, a.positions[1], leaf, 0, Mode::Custody);
    let before = h.snapshot(&a);
    let mut foreign = good.clone();
    foreign
        .accounts
        .iter_mut()
        .find(|m| m.pubkey == a.positions[1])
        .unwrap()
        .pubkey = b.positions[1];
    custom_error(h.run(1, &[foreign]), CustodyError::InvalidNode);
    let pristine = h.svm.get_account(&a.positions[1]).unwrap();
    for fault in 0..3 {
        let mut raw = pristine.clone();
        match fault {
            0 => raw.owner = SYSTEM,
            _ => {
                let mut state = Position::try_deserialize(&mut raw.data.as_slice()).unwrap();
                if fault == 1 {
                    state.owner = ap(h.keys[0].pubkey());
                } else {
                    state.version = 255;
                }
                state.try_serialize(&mut raw.data.as_mut_slice()).unwrap();
            }
        }
        h.svm.set_account(a.positions[1], raw).unwrap();
        assert!(h.run(1, std::slice::from_ref(&good)).is_err());
    }
    h.svm.set_account(a.positions[1], pristine).unwrap();
    assert_eq!(h.snapshot(&a), before);
    let journal = h.journal();
    let debit = h.node(journal, journal, NodeKind::DebitLeaf, 1);
    custom_error(
        h.run(1, &[h.posting(journal, debit, debit, 0, Mode::Custody)]),
        CustodyError::UnsupportedRoot,
    );
    let mut substituted = good;
    substituted.accounts[1].pubkey = journal;
    custom_error(h.run(1, &[substituted]), CustodyError::UnsupportedRoot);
}
