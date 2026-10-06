//! Actual sBPF mapping path, with no physical leaf accounts.
use anchor_lang::{prelude::Pubkey, InstructionData, ToAccountMetas};
use cavalre_ledger_solana::{
    self as ledger, accounts, instruction, ledger_lib as lib, ledger_storage as map,
};
use litesvm::LiteSVM;
use solana_address::Address;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
fn address(key: Pubkey) -> Address {
    Address::new_from_array(key.to_bytes())
}
fn pk(key: Address) -> Pubkey {
    Pubkey::new_from_array(key.to_bytes())
}
fn instruction(a: impl ToAccountMetas, d: impl InstructionData, rest: &[Pubkey]) -> Instruction {
    let mut accounts: Vec<_> = a
        .to_account_metas(None)
        .into_iter()
        .map(|m| AccountMeta {
            pubkey: address(m.pubkey),
            is_signer: m.is_signer,
            is_writable: m.is_writable,
        })
        .collect();
    accounts.extend(rest.iter().map(|k| AccountMeta::new(address(*k), false)));
    Instruction {
        program_id: address(ledger::ID),
        accounts,
        data: d.data(),
    }
}
#[test]
fn mapping_mint_transfer_and_burn_with_no_leaf_pdas() {
    let mut svm = LiteSVM::new();
    svm.add_program(
        address(ledger::ID),
        &std::fs::read("../../target/deploy/cavalre_ledger_solana.so").unwrap(),
    )
    .unwrap();
    let signer = Keypair::new_from_array([1; 32]);
    svm.airdrop(&signer.pubkey(), 10_000_000_000).unwrap();
    let owner = pk(signer.pubkey());
    let id = Pubkey::new_from_array([100; 32]);
    let root = lib::root_storage_address(&owner, &id).0;
    let mut run = |ix: Instruction| {
        svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            &[ix],
            Some(&signer.pubkey()),
            &[&signer],
            svm.latest_blockhash(),
        );
        svm.send_transaction(tx).unwrap()
    };
    run(instruction(
        accounts::RegisterLedger {
            payer: owner,
            authority: owner,
            root,
            system_program: Pubkey::default(),
            global_root: lib::GLOBAL_ROOT,
        },
        instruction::AddLedger {
            id,
            name: "Test".into(),
            symbol: "TEST".into(),
            decimals: 6,
        },
        &[],
    ));
    let app = lib::to_address(&root, &owner);
    let store = lib::account_storage_address(&root, &owner).0;
    let base = accounts::LedgerAccounts {
        payer: owner,
        authority: owner,
        root,
        system_program: Pubkey::default(),
    };
    let mut create = instruction(
        base,
        instruction::AddSubAccountGroup {
            parent: root,
            relative: owner,
            name: "App".into(),
            credit: false,
            implicit_allowed: true,
        },
        &[store],
    );
    create.accounts[2].is_writable = true;
    run(create);
    let alice = Pubkey::new_from_array([2; 32]);
    let bob = Pubkey::new_from_array([3; 32]);
    for (from_parent, from, to_parent, to, amount) in [
        (root, lib::SOURCE, app, alice, 100),
        (app, alice, app, bob, 10),
        (app, bob, root, lib::SOURCE, 10),
    ] {
        let base = accounts::LedgerAccounts {
            payer: owner,
            authority: owner,
            root,
            system_program: Pubkey::default(),
        };
        let mut ix = instruction(
            base,
            instruction::Transfer {
                from_parent,
                from,
                to_parent,
                to,
                amount,
            },
            &[store],
        );
        ix.accounts[2].is_writable = from_parent == root || to_parent == root;
        let result = run(ix);
        eprintln!(
            "mapping transfer {amount}: {} CU",
            result.compute_units_consumed
        );
    }
    let container = svm.get_account(&address(store)).unwrap();
    for (relative, balance) in [(alice, 90), (bob, 0)] {
        let key = lib::to_address(&app, &relative);
        let Some(map::Value::Leaf(record)) = map::get(&container.data, &key).unwrap() else {
            panic!("missing leaf");
        };
        assert_eq!(record.debit, balance);
        assert!(!record.registered);
        assert!(svm.get_account(&address(key)).is_none());
        assert!(svm
            .get_account(&address(lib::account_storage_address(&app, &relative).0))
            .is_none());
    }
}
