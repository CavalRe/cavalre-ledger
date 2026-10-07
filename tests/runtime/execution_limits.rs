//! Real sBPF with signed legacy packets, or v0/ALT when legacy exceeds wire size.
use super::*;
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_message::{v0, AddressLookupTableAccount, VersionedMessage};
use solana_transaction::versioned::VersionedTransaction;

const PACKET_BYTES: usize = 1232;
const COMPUTE_UNITS: u32 = 300_000;
// Sampling range for these transaction shapes, not a Ledger policy.
const PROFILE_DEPTHS: std::ops::RangeInclusive<u8> = 4..=13;

struct Profile {
    h: Harness,
    rows: Vec<serde_json::Value>,
    mode: &'static str,
    depth: u8,
    seed: u8,
    app: Option<Address>,
    authority: Address,
    lookup: Option<AddressLookupTableAccount>,
}
impl Profile {
    fn new(mode: &'static str, depth: u8, seed: u8) -> Self {
        let mut h = Harness::new();
        let app = (mode == "cpi").then(|| Address::new_from_array([180 + seed; 32]));
        let authority = if let Some(app) = app {
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
            h.key(if mode == "internal" { 0 } else { 2 })
        };
        Self {
            h,
            rows: Vec::new(),
            mode,
            depth,
            seed,
            app,
            authority,
            lookup: None,
        }
    }

    fn run(&mut self, operation: &str, inner: Instruction, funding: bool) -> bool {
        let inner = self.h.indexed(inner);
        let instruction = if let Some(app) = self.app {
            proxy(&self.h, app, self.authority, inner)
        } else {
            inner
        };
        self.h.svm.expire_blockhash();
        let mut signers = vec![&self.h.keys[0]];
        if funding {
            signers.push(&self.h.keys[1]);
        }
        if self.mode == "direct" {
            signers.push(&self.h.keys[2]);
        }
        let mut instructions = vec![
            ComputeBudgetInstruction::set_compute_unit_limit(COMPUTE_UNITS),
            ComputeBudgetInstruction::set_compute_unit_price(0),
        ];
        instructions.extend(self.h.prepared(instruction));
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&self.h.key(0)),
            &signers,
            self.h.svm.latest_blockhash(),
        );
        let legacy_bytes = wincode::serialize(&tx).unwrap().len();
        let accounts = tx.message.account_keys.len();
        let writable = (0..accounts)
            .filter(|i| {
                tx.message.is_maybe_writable_with_reserved_addresses(
                    *i,
                    None::<&std::collections::BTreeSet<Address>>,
                )
            })
            .count();
        let account_keys = tx.message.account_keys.clone();
        let mut setup_fee = 0;
        let mut setup_rent = 0;
        let mut setup_compute = 0;
        let tx =
            if legacy_bytes <= PACKET_BYTES {
                VersionedTransaction::from(tx)
            } else {
                use solana_address_lookup_table_interface::instruction::{
                    create_lookup_table, extend_lookup_table,
                };
                // Exercise the real lookup-table program, including warm-up. Setup
                // costs are separate from the measured Ledger operation, not hidden.
                let payer = self.h.key(0);
                let payer_before = self.h.svm.get_account(&payer).unwrap().lamports;
                let slot = self.h.svm.get_sysvar::<anchor_lang::prelude::Clock>().slot;
                let mut setup = Vec::new();
                let mut table = self.lookup.take().unwrap_or_else(|| {
                    let (create, key) = create_lookup_table(payer, payer, slot);
                    setup.push(create);
                    AddressLookupTableAccount {
                        key,
                        addresses: Vec::new(),
                    }
                });
                let addresses = account_keys
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !tx.message.is_signer(*index))
                    .map(|(_, address)| *address)
                    .filter(|address| !table.addresses.contains(address))
                    .collect::<Vec<_>>();
                setup.extend(addresses.chunks(20).map(|chunk| {
                    extend_lookup_table(table.key, payer, Some(payer), chunk.to_vec())
                }));
                table.addresses.extend(addresses);
                for instruction in setup {
                    let tx = Transaction::new_signed_with_payer(
                        &[instruction],
                        Some(&payer),
                        &[&self.h.keys[0]],
                        self.h.svm.latest_blockhash(),
                    );
                    assert!(wincode::serialize(&tx).unwrap().len() <= PACKET_BYTES);
                    let result = self.h.svm.send_transaction(tx).unwrap();
                    setup_fee += result.fee;
                    setup_compute += result.compute_units_consumed;
                }
                setup_rent =
                    payer_before - self.h.svm.get_account(&payer).unwrap().lamports - setup_fee;
                self.h.svm.warp_to_slot(slot + 1);
                let message = v0::Message::try_compile(
                    &payer,
                    &instructions,
                    std::slice::from_ref(&table),
                    self.h.svm.latest_blockhash(),
                )
                .unwrap();
                self.lookup = Some(table);
                let mut signers = vec![&self.h.keys[0]];
                if funding {
                    signers.push(&self.h.keys[1]);
                }
                if self.mode == "direct" {
                    signers.push(&self.h.keys[2]);
                }
                VersionedTransaction::try_new(VersionedMessage::V0(message), &signers).unwrap()
            };
        let bytes = wincode::serialize(&tx).unwrap().len();
        let before: Vec<_> = account_keys
            .iter()
            .map(|key| (*key, self.h.svm.get_account(key)))
            .collect();
        let mut row = serde_json::json!({"mode":self.mode,"depth":self.depth,"seed":self.seed,
            "operation":operation,"bytes":bytes,"legacy_bytes":legacy_bytes,
            "format":if legacy_bytes > PACKET_BYTES {"v0"} else {"legacy"},
            "lookup_setup_fee_lamports":setup_fee,"lookup_setup_rent_lamports":setup_rent,
            "lookup_setup_compute_units":setup_compute,"accounts":accounts,"writable":writable});
        // LiteSVM is not a network packet admission test. Enforce wire size here.
        if bytes > PACKET_BYTES {
            row["status"] = "packet_too_large".into();
            self.rows.push(row);
            return false;
        }
        let result = self.h.svm.send_transaction(tx);
        let (meta, status) = match &result {
            Ok(meta) => (meta, "ok".to_string()),
            Err(error) => (&error.meta, format!("{:?}", error.err)),
        };
        row["compute_units"] = meta.compute_units_consumed.into();
        row["fee_lamports"] = meta.fee.into();
        row["status"] = status.into();
        if result.is_err() {
            row["logs"] = serde_json::json!(meta.logs);
        }
        let old_payer = before
            .iter()
            .find(|(key, _)| *key == self.h.key(0))
            .unwrap()
            .1
            .as_ref()
            .unwrap()
            .lamports;
        let rent = i128::from(old_payer)
            - i128::from(self.h.svm.get_account(&self.h.key(0)).unwrap().lamports)
            - i128::from(meta.fee);
        row["rent_lamports"] = i64::try_from(rent).unwrap().into();
        let mut allocated_rent = 0i128;
        for (key, old) in before {
            let mut new = self.h.svm.get_account(&key);
            if result.is_err() {
                if key == self.h.key(0) {
                    new.as_mut().unwrap().lamports += meta.fee;
                }
                assert_eq!(new, old, "failed profile changed {key}");
            } else if key != self.h.key(0) {
                let previous = old.as_ref().map_or(0, |a| a.lamports);
                let current = new.as_ref().map_or(0, |a| a.lamports);
                if new.as_ref().is_some_and(|a| a.owner == sa(ledger::ID))
                    || old.as_ref().is_some_and(|a| a.owner == sa(ledger::ID))
                {
                    allocated_rent += i128::from(current) - i128::from(previous);
                }
            }
        }
        if result.is_ok() {
            assert_eq!(rent, allocated_rent);
        }
        self.rows.push(row);
        result.is_ok()
    }

    fn group(
        &mut self,
        root: Address,
        parent: Address,
        relative: Address,
        path: &[Address],
    ) -> bool {
        let mut rest = path.to_vec();
        rest.extend([parent, child(parent, relative)]);
        self.run(
            "create_group",
            ix(
                base(&self.h, root, self.authority),
                instruction::AddSubAccountGroup {
                    parent: ap(parent),
                    relative: ap(relative),
                    name: "G".repeat(64),
                    credit: false,
                },
                &remaining(root, &rest),
            ),
            false,
        )
    }

    fn register_leaf(
        &mut self,
        root: Address,
        parent: Address,
        relative: Address,
        path: &[Address],
    ) -> bool {
        let mut create = leaf(
            &self.h,
            root,
            self.authority,
            parent,
            relative,
            &"L".repeat(64),
            false,
        );
        let existing: Vec<_> = create.accounts.iter().map(|a| a.pubkey).collect();
        create.accounts.extend(
            path.iter()
                .filter(|k| !existing.contains(k))
                .map(|k| AccountMeta::new(*k, false)),
        );
        self.run("register_funded_leaf", create, false)
    }

    fn exercise(&mut self) {
        let external = (self.mode != "internal")
            .then(|| External::named(&mut self.h, 100 + self.seed * 2, 1, &"T".repeat(32)));
        let (root, source) = external
            .as_ref()
            .map(|e| (e.root, e.source))
            .unwrap_or_else(|| self.h.internal_named(&"R".repeat(64)));
        // External branches diverge immediately below their common custodian.
        // Internal branches diverge at the root, exercising the longer walk.
        let mut common = root;
        let mut common_path = vec![];
        if external.is_some() {
            if !self.group(root, root, self.authority, &[]) {
                return;
            }
            common = child(root, self.authority);
            common_path.push(common);
        }
        let mut paths = [common_path.clone(), common_path];
        let mut parents = [common; 2];
        for side in 0..2 {
            let start = if external.is_some() { 4 } else { 3 };
            for level in start..self.depth {
                let mut bytes = [self.seed; 32];
                bytes[0] = side as u8 + 20;
                bytes[1] = level;
                let relative = Address::new_from_array(bytes);
                if !self.group(root, parents[side], relative, &paths[side]) {
                    return;
                }
                parents[side] = child(parents[side], relative);
                paths[side].push(parents[side]);
            }
        }
        let a = Address::new_from_array([210; 32]);
        let b = Address::new_from_array([211; 32]);
        let registered = Address::new_from_array([212; 32]);
        let mut create = leaf(
            &self.h,
            root,
            self.authority,
            parents[0],
            registered,
            &"L".repeat(64),
            false,
        );
        let existing: Vec<_> = create.accounts.iter().map(|a| a.pubkey).collect();
        create.accounts.extend(
            paths[0]
                .iter()
                .filter(|k| !existing.contains(k))
                .map(|k| AccountMeta::new(*k, false)),
        );
        if !self.run("create_leaf", create, false) {
            return;
        }
        let mut delete = remove(&self.h, root, self.authority, parents[0], registered, false);
        let existing: Vec<_> = delete.accounts.iter().map(|a| a.pubkey).collect();
        delete.accounts.extend(
            paths[0]
                .iter()
                .filter(|k| !existing.contains(k))
                .map(|k| AccountMeta::new(*k, false)),
        );
        if !self.run("remove_leaf", delete, false) {
            return;
        }

        for label in ["deposit_first", "deposit_repeat"] {
            let i = if let Some(e) = &external {
                e.movement(
                    &self.h,
                    (self.authority, 1),
                    (parents[0], a),
                    100,
                    true,
                    &paths[0],
                )
            } else {
                let mut path = paths[0].clone();
                path.push(source);
                transfer(
                    &self.h,
                    root,
                    self.authority,
                    (root, sa(SOURCE)),
                    (parents[0], a),
                    100,
                    &path,
                )
            };
            if !self.run(label, i, external.is_some()) {
                return;
            }
            assert!(self.h.record(child(parents[0], a)).child_index > 0);
        }
        let mut all = paths[0].clone();
        all.extend(&paths[1]);
        for label in ["transfer_first", "transfer_repeat"] {
            let i = transfer(
                &self.h,
                root,
                self.authority,
                (parents[0], a),
                (parents[1], b),
                40,
                &all,
            );
            if !self.run(label, i, false) {
                return;
            }
        }
        if !self.register_leaf(root, parents[0], a, &paths[0])
            || !self.register_leaf(root, parents[1], b, &paths[1])
        {
            return;
        }
        let i = transfer(
            &self.h,
            root,
            self.authority,
            (parents[0], a),
            (parents[1], b),
            1,
            &all,
        );
        if !self.run("transfer_registered", i, false) {
            return;
        }
        let i = if let Some(e) = &external {
            e.movement(
                &self.h,
                (self.authority, 1),
                (parents[0], a),
                1,
                true,
                &paths[0],
            )
        } else {
            let mut path = paths[0].clone();
            path.push(source);
            transfer(
                &self.h,
                root,
                self.authority,
                (root, sa(SOURCE)),
                (parents[0], a),
                1,
                &path,
            )
        };
        if !self.run("deposit_registered", i, external.is_some()) {
            return;
        }
        let i = if let Some(e) = &external {
            e.movement(
                &self.h,
                (self.authority, 0),
                (parents[1], b),
                11,
                false,
                &paths[1],
            )
        } else {
            let mut path = paths[1].clone();
            path.push(source);
            transfer(
                &self.h,
                root,
                self.authority,
                (parents[1], b),
                (root, sa(SOURCE)),
                11,
                &path,
            )
        };
        if !self.run("withdraw", i, false) {
            return;
        }
        assert_eq!(self.h.record(child(parents[0], a)).debit, 120);
        assert_eq!(self.h.record(child(parents[1], b)).debit, 70);
        assert_eq!(self.h.record(source).credit, 190);
        let totals = self.h.record(root);
        assert_eq!((totals.debit, totals.credit), (190, 190));
        if let Some(e) = external {
            assert_eq!((self.h.token(e.vault), self.h.token(e.wallet)), (190, 810));
        } else {
            // Opposite-polarity endpoints update the common root too. Both long
            // paths must fit, including first-use and registered credit leaves.
            let mut parent = root;
            let mut credit_path = vec![];
            for level in 3..self.depth {
                let mut bytes = [self.seed; 32];
                bytes[0] = 25;
                bytes[1] = level;
                let relative = Address::new_from_array(bytes);
                let mut rest = credit_path.clone();
                rest.extend([parent, child(parent, relative)]);
                let i = ix(
                    base(&self.h, root, self.authority),
                    instruction::AddSubAccountGroup {
                        parent: ap(parent),
                        relative: ap(relative),
                        name: "C".repeat(64),
                        credit: true,
                    },
                    &remaining(root, &rest),
                );
                if !self.run("create_credit_group", i, false) {
                    return;
                }
                parent = child(parent, relative);
                credit_path.push(parent);
            }
            credit_path.extend(&paths[0]);
            for label in ["issue_deep_first", "issue_deep_registered"] {
                if label == "issue_deep_registered" {
                    let mut i = leaf(
                        &self.h,
                        root,
                        self.authority,
                        parent,
                        b,
                        &"C".repeat(64),
                        true,
                    );
                    let existing: Vec<_> = i.accounts.iter().map(|a| a.pubkey).collect();
                    i.accounts.extend(
                        credit_path
                            .iter()
                            .filter(|k| !existing.contains(k))
                            .map(|k| AccountMeta::new(*k, false)),
                    );
                    if !self.run("register_credit_leaf", i, false) {
                        return;
                    }
                }
                let i = transfer(
                    &self.h,
                    root,
                    self.authority,
                    (parent, b),
                    (parents[0], a),
                    1,
                    &credit_path,
                );
                if !self.run(label, i, false) {
                    return;
                }
            }
            let i = transfer(
                &self.h,
                root,
                self.authority,
                (parents[0], a),
                (parent, b),
                2,
                &credit_path,
            );
            if !self.run("retire_deep", i, false) {
                return;
            }
            assert_eq!(self.h.record(child(parent, b)).credit, 0);
            assert_eq!(self.h.record(child(parents[0], a)).debit, 120);
            assert_eq!(self.h.record(root).credit, 190);
            assert_eq!(self.h.record(root).debit, 190);
        }
    }
}

#[test]
fn execution_profiles() {
    let mut rows = Vec::new();
    let mut completed = true;
    for depth in PROFILE_DEPTHS {
        for mode in ["internal", "direct", "cpi"] {
            for seed in 0..3 {
                let mut profile = Profile::new(mode, depth, seed);
                profile.exercise();
                completed &= profile.rows.last().unwrap()["operation"]
                    == if mode == "internal" {
                        "retire_deep"
                    } else {
                        "withdraw"
                    };
                rows.extend(profile.rows);
            }
        }
    }
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("execution-profile.json"),
        serde_json::to_string_pretty(&rows).unwrap(),
    )
    .unwrap();
    // Save diagnostics even on failure so callers can inspect resource usage.
    let failures: Vec<_> = rows.iter().filter(|r| r["status"] != "ok").collect();
    assert!(failures.is_empty(), "profile failures: {failures:?}");
    assert!(completed, "profile did not execute every operation");
    assert!(rows
        .iter()
        .all(|r| r["compute_units"].as_u64().unwrap() * 11 <= u64::from(COMPUTE_UNITS) * 10));
}

#[test]
fn deeper_trees_allow_creation_conversion_and_posting() {
    for mode in ["internal", "direct", "cpi"] {
        let mut p = Profile::new(mode, 14, 0);
        let external = (mode != "internal").then(|| External::new(&mut p.h, 100, 1));
        let (root, source) = external
            .as_ref()
            .map(|e| (e.root, e.source))
            .unwrap_or_else(|| p.h.internal());
        let mut parent = root;
        let mut path = Vec::new();
        for depth in 3..=13 {
            let relative = if depth == 3 {
                p.authority
            } else {
                Address::new_from_array([depth; 32])
            };
            assert!(p.group(root, parent, relative, &path));
            parent = child(parent, relative);
            path.push(parent);
        }
        assert_eq!(p.h.record(parent).depth, 13);
        let relative = Address::new_from_array([90; 32]);
        assert!(p.register_leaf(root, parent, relative, &path));
        assert_eq!(p.h.record(child(parent, relative)).depth, 14);
        // An empty unregistered leaf can also become a group at this depth.
        let i = remove(&p.h, root, p.authority, parent, relative, false);
        let mut i = i;
        let existing: Vec<_> = i.accounts.iter().map(|a| a.pubkey).collect();
        i.accounts.extend(
            path.iter()
                .filter(|k| !existing.contains(k))
                .map(|k| AccountMeta::new(*k, false)),
        );
        assert!(p.run("remove_leaf", i, false));
        assert!(p.group(root, parent, relative, &path));
        assert_eq!(p.h.record(child(parent, relative)).kind, 0);
        assert_eq!(p.h.record(parent).children.len(), 1);

        let a = Address::new_from_array([91; 32]);
        let b = Address::new_from_array([92; 32]);
        let i = if let Some(e) = &external {
            e.movement(&p.h, (p.authority, 1), (parent, a), 100, true, &path)
        } else {
            let mut funding_path = path.clone();
            funding_path.push(source);
            transfer(
                &p.h,
                root,
                p.authority,
                (root, sa(SOURCE)),
                (parent, a),
                100,
                &funding_path,
            )
        };
        assert!(p.run("deposit_first", i, external.is_some()));
        let i = transfer(&p.h, root, p.authority, (parent, a), (parent, b), 20, &path);
        assert!(p.run("transfer_first", i, false));
        let from = p.h.record(child(parent, a));
        let to = p.h.record(child(parent, b));
        assert_eq!((from.depth, to.depth), (14, 14));
        assert!(from.child_index > 0 && to.child_index > 0);
        assert_eq!((from.debit, to.debit), (80, 20));
        assert_eq!(p.h.record(source).credit, 100);
        assert_eq!(
            (p.h.record(root).debit, p.h.record(root).credit),
            (100, 100)
        );
    }
}
