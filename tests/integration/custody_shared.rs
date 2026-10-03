// Tests load the actual sBPF artifact; there is no native program substitute.
use anchor_lang::{AccountDeserialize, AccountSerialize, InstructionData, ToAccountMetas};
use crate::ledger as cavalre;
use cavalre::{accounts, instruction, Asset, CustodyError, Ledger, Position};
use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use solana_account::Account;
use solana_address::{address, Address};
use solana_compute_budget_interface::ComputeBudgetInstruction;
use solana_instruction::{AccountMeta, Instruction};
use solana_instruction_error::InstructionError;
use solana_keypair::Keypair;
use solana_program_option::COption;
use solana_program_pack::Pack;
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;
use spl_token_interface::state::{Account as TokenAccount, AccountState, Mint};
use std::{collections::BTreeSet, path::PathBuf};

const INITIAL_TOKENS: u64 = 10_000;
const SYSTEM: Address = address!("11111111111111111111111111111111");
const TOKEN_2022: Address = address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const PACKET_BYTES: usize = 1232;
type TransactionResult = Result<TransactionMetadata, Box<FailedTransactionMetadata>>;

fn ap(key: Address) -> anchor_lang::prelude::Pubkey {
    anchor_lang::prelude::Pubkey::new_from_array(key.to_bytes())
}

fn sa(key: anchor_lang::prelude::Pubkey) -> Address {
    Address::new_from_array(key.to_bytes())
}

fn pda(seeds: &[&[u8]]) -> Address {
    sa(anchor_lang::prelude::Pubkey::find_program_address(seeds, &cavalre::ID).0)
}

fn ix(a: impl ToAccountMetas, data: impl InstructionData) -> Instruction {
    Instruction {
        program_id: sa(cavalre::ID),
        accounts: a
            .to_account_metas(None)
            .into_iter()
            .map(|m| AccountMeta {
                pubkey: Address::new_from_array(m.pubkey.to_bytes()),
                is_signer: m.is_signer,
                is_writable: m.is_writable,
            })
            .collect(),
        data: data.data(),
    }
}

#[derive(Clone)]
struct AssetFixture {
    mint: Address,
    asset: Address,
    vault: Address,
    wallets: [Address; 3],
    positions: [Address; 3],
}

struct Harness {
    svm: LiteSVM,
    keys: [Keypair; 3],
    ledger: Address,
    next_key: u8,
}

impl Harness {
    fn new() -> Self {
        let mut svm = LiteSVM::new();
        let so = std::env::var_os(PROGRAM_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PROGRAM_SO)
            });
        let bytes = std::fs::read(&so).unwrap_or_else(|e| {
            panic!("Build sBPF first (scripts/check.sh): {}: {e}", so.display())
        });
        svm.add_program(sa(cavalre::ID), &bytes).unwrap();
        // Public, deterministic simulator identities, never deployment keys.
        let keys = std::array::from_fn(|i| Keypair::new_from_array([i as u8 + 1; 32]));
        for k in &keys {
            svm.airdrop(&k.pubkey(), 10_000_000_000).unwrap();
        }
        let ledger = pda(&[b"ledger", keys[0].pubkey().as_ref(), &0u64.to_le_bytes()]);
        let mut h = Self {
            svm,
            keys,
            ledger,
            next_key: 10,
        };
        h.run(0, &[h.initialize_ix(0)]).unwrap();
        h
    }

    fn initialize_ix(&self, id: u64) -> Instruction {
        ix(
            accounts::InitializeLedger {
                authority: ap(self.keys[0].pubkey()),
                ledger: ap(pda(&[
                    b"ledger",
                    self.keys[0].pubkey().as_ref(),
                    &id.to_le_bytes(),
                ])),
                system_program: ap(SYSTEM),
            },
            instruction::InitializeLedger { id },
        )
    }

    fn address(&mut self) -> Address {
        let key = Keypair::new_from_array([self.next_key; 32]).pubkey();
        self.next_key = self.next_key.checked_add(1).unwrap();
        key
    }

    fn run(&mut self, signer: usize, instructions: &[Instruction]) -> TransactionResult {
        self.svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.keys[signer].pubkey()),
            &[&self.keys[signer]],
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx).map_err(Box::new)
    }

    fn put_packed<T: Pack>(&mut self, key: Address, value: T) {
        let mut data = vec![0; T::LEN];
        T::pack(value, &mut data).unwrap();
        self.svm
            .set_account(
                key,
                Account {
                    lamports: self.svm.minimum_balance_for_rent_exemption(T::LEN),
                    data,
                    owner: spl_token_interface::ID,
                    executable: false,
                    rent_epoch: u64::MAX,
                },
            )
            .unwrap();
    }

    fn asset(&mut self) -> AssetFixture {
        let mint = self.address();
        self.put_packed(
            mint,
            Mint {
                mint_authority: COption::None,
                supply: INITIAL_TOKENS * 3,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::Some(self.keys[0].pubkey()),
            },
        );
        let asset = pda(&[b"asset", self.ledger.as_ref(), mint.as_ref()]);
        let vault = pda(&[b"vault", asset.as_ref()]);
        let wallets = std::array::from_fn(|i| {
            let wallet = self.address();
            self.put_packed(
                wallet,
                TokenAccount {
                    mint,
                    owner: self.keys[i].pubkey(),
                    amount: INITIAL_TOKENS,
                    delegate: COption::None,
                    state: AccountState::Initialized,
                    is_native: COption::None,
                    delegated_amount: 0,
                    close_authority: COption::None,
                },
            );
            wallet
        });
        let positions = std::array::from_fn(|i| {
            pda(&[b"position", asset.as_ref(), self.keys[i].pubkey().as_ref()])
        });
        let a = AssetFixture {
            mint,
            asset,
            vault,
            wallets,
            positions,
        };
        self.run(0, &[self.register_ix(&a, 0)]).unwrap();
        for i in 0..3 {
            self.run(i, &[self.open_ix(&a, i)]).unwrap();
        }
        a
    }

    fn register_ix(&self, a: &AssetFixture, authority: usize) -> Instruction {
        ix(
            accounts::RegisterAsset {
                authority: ap(self.keys[authority].pubkey()),
                ledger: ap(self.ledger),
                mint: ap(a.mint),
                asset: ap(a.asset),
                vault: ap(a.vault),
                token_program: ap(spl_token_interface::ID),
                system_program: ap(SYSTEM),
            },
            instruction::RegisterAsset {},
        )
    }

    fn open_ix(&self, a: &AssetFixture, user: usize) -> Instruction {
        ix(
            accounts::OpenPosition {
                owner: ap(self.keys[user].pubkey()),
                asset: ap(a.asset),
                position: ap(a.positions[user]),
                system_program: ap(SYSTEM),
            },
            instruction::OpenPosition {},
        )
    }

    fn move_ix(&self, a: &AssetFixture, user: usize, amount: u64, deposit: bool) -> Instruction {
        let a = accounts::MoveTokens {
            owner: ap(self.keys[user].pubkey()),
            asset: ap(a.asset),
            position: ap(a.positions[user]),
            mint: ap(a.mint),
            vault: ap(a.vault),
            wallet: ap(a.wallets[user]),
            token_program: ap(spl_token_interface::ID),
        };
        if deposit {
            ix(a, instruction::Deposit { amount })
        } else {
            ix(a, instruction::Withdraw { amount })
        }
    }

    fn close_ix(&self, a: &AssetFixture, user: usize) -> Instruction {
        ix(
            accounts::ClosePosition {
                owner: ap(self.keys[user].pubkey()),
                position: ap(a.positions[user]),
            },
            instruction::ClosePosition {},
        )
    }

    fn transfer_ix(&self, a: &AssetFixture, from: usize, to: usize, amount: u64) -> Instruction {
        ix(
            accounts::TransferClaims {
                owner: ap(self.keys[from].pubkey()),
                asset: ap(a.asset),
                source: ap(a.positions[from]),
                destination: ap(a.positions[to]),
            },
            instruction::TransferClaims { amount },
        )
    }

    fn state<T: AccountDeserialize>(&self, key: Address) -> T {
        T::try_deserialize(&mut self.svm.get_account(&key).unwrap().data.as_slice()).unwrap()
    }

    fn token(&self, key: Address) -> TokenAccount {
        TokenAccount::unpack(&self.svm.get_account(&key).unwrap().data).unwrap()
    }

    fn snapshot(&self, a: &AssetFixture) -> Vec<Option<Account>> {
        [a.asset, a.vault, a.mint]
            .into_iter()
            .chain(a.positions)
            .chain(a.wallets)
            .map(|k| self.svm.get_account(&k))
            .collect()
    }

    fn assert_conservation(&self, a: &AssetFixture) {
        let sum: u128 = a
            .positions
            .iter()
            .map(|k| self.state::<Position>(*k).balance as u128)
            .sum();
        assert_eq!(self.state::<Asset>(a.asset).total_claims as u128, sum);
        assert!(self.token(a.vault).amount as u128 >= sum);
        let supply = Mint::unpack(&self.svm.get_account(&a.mint).unwrap().data)
            .unwrap()
            .supply;
        let external: u128 = a
            .wallets
            .iter()
            .map(|k| self.token(*k).amount as u128)
            .sum();
        assert_eq!(
            external + self.token(a.vault).amount as u128,
            supply as u128
        );
    }
}

fn custom_error(result: TransactionResult, error: CustodyError) {
    assert_eq!(
        result.unwrap_err().err,
        TransactionError::InstructionError(0, InstructionError::Custom(6000 + error as u32))
    );
}

#[test]
fn replays_executed_solidity_custody_fixture_in_sbpf() {
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/fixtures/custody.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let pin: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/upstream.json"
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(fixture["contracts_commit"], pin["references"][0]["commit"]);
    assert_eq!(
        fixture["initial_tokens_per_user"].as_u64().unwrap(),
        INITIAL_TOKENS
    );
    let mut h = Harness::new();
    let a = h.asset();
    for (i, step) in fixture["steps"].as_array().unwrap().iter().enumerate() {
        let user = 1 + step["user"].as_u64().unwrap() as usize;
        let amount = step["amount"].as_u64().unwrap();
        let kind = step["kind"].as_u64().unwrap();
        let instruction = match kind {
            0 | 1 => h.move_ix(&a, user, amount, kind == 0),
            2 => spl_token_interface::instruction::transfer_checked(
                &spl_token_interface::ID,
                &a.wallets[user],
                &a.mint,
                &a.vault,
                &h.keys[user].pubkey(),
                &[],
                amount,
                6,
            )
            .unwrap(),
            _ => panic!("unsupported reference action"),
        };
        let before = h.snapshot(&a);
        let result = h.run(user, &[instruction]);
        assert_eq!(
            result.is_ok(),
            step["success"].as_bool().unwrap(),
            "step {i}: {result:?}"
        );
        if result.is_err() {
            assert_eq!(h.snapshot(&a), before, "rollback at step {i}");
        }
        for u in 0..2 {
            assert_eq!(
                h.state::<Position>(a.positions[u + 1]).balance,
                step["positions"][u].as_u64().unwrap(),
                "step {i}"
            );
            assert_eq!(
                h.token(a.wallets[u + 1]).amount,
                step["wallets"][u].as_u64().unwrap(),
                "step {i}"
            );
        }
        assert_eq!(
            h.state::<Asset>(a.asset).total_claims,
            step["total_claims"].as_u64().unwrap(),
            "step {i}"
        );
        assert_eq!(
            h.token(a.vault).amount,
            step["vault"].as_u64().unwrap(),
            "step {i}"
        );
        h.assert_conservation(&a);
    }
}

#[test]
fn claim_transfer_changes_the_authorized_withdrawer_without_touching_custody_or_supply() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let asset = h.svm.get_account(&a.asset);
    let vault = h.svm.get_account(&a.vault);
    let instruction = h.transfer_ix(&a, 1, 2, 45);
    assert_eq!(
        instruction
            .accounts
            .iter()
            .filter(|a| a.is_writable)
            .count(),
        2
    );
    h.run(1, &[instruction]).unwrap();
    assert_eq!(h.svm.get_account(&a.asset), asset);
    assert_eq!(h.svm.get_account(&a.vault), vault);
    assert_eq!(h.state::<Position>(a.positions[1]).balance, 75);
    assert_eq!(h.state::<Position>(a.positions[2]).balance, 45);
    custom_error(
        h.run(1, &[h.move_ix(&a, 1, 76, false)]),
        CustodyError::InsufficientBalance,
    );
    h.run(2, &[h.move_ix(&a, 2, 45, false)]).unwrap();
    h.assert_conservation(&a);
    assert_eq!(h.token(a.wallets[2]).amount, INITIAL_TOKENS + 45);
}

#[test]
fn self_claim_transfers_preserve_balances_and_check_spendable_amount() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let before = h.snapshot(&a);
    for amount in [0, 1, 120] {
        h.run(1, &[h.transfer_ix(&a, 1, 1, amount)]).unwrap();
    }
    custom_error(
        h.run(1, &[h.transfer_ix(&a, 1, 1, 121)]),
        CustodyError::InsufficientBalance,
    );
    assert_eq!(h.snapshot(&a), before);
}

#[test]
fn claim_transfers_reject_wrong_owner_and_cross_asset_destinations_and_roll_back() {
    let mut h = Harness::new();
    let a = h.asset();
    let b = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let before = h.snapshot(&a);
    let mut forged = h.transfer_ix(&a, 1, 2, 10);
    forged.accounts[0].pubkey = h.keys[0].pubkey();
    assert!(h.run(0, &[forged]).is_err());
    let mut forged = h.transfer_ix(&a, 1, 2, 10);
    forged.accounts[3].pubkey = b.positions[2];
    assert!(h.run(1, &[forged]).is_err());
    let err = h
        .run(
            1,
            &[h.transfer_ix(&a, 1, 2, 100), h.transfer_ix(&a, 1, 2, 21)],
        )
        .unwrap_err();
    assert_eq!(
        err.err,
        TransactionError::InstructionError(
            1,
            InstructionError::Custom(6000 + CustodyError::InsufficientBalance as u32)
        )
    );
    assert_eq!(h.snapshot(&a), before);
}

#[test]
fn custody_lifecycle_conserves_claims_and_refunds_position_rent() {
    let mut h = Harness::new();
    let a = h.asset();
    let ledger = h.state::<Ledger>(h.ledger);
    assert_eq!(ledger.authority, ap(h.keys[0].pubkey()));
    assert_eq!(h.svm.get_account(&h.ledger).unwrap().data.len(), 50);
    assert_eq!(h.svm.get_account(&a.asset).unwrap().data.len(), 83);
    assert_eq!(h.svm.get_account(&a.positions[1]).unwrap().data.len(), 82);
    assert_eq!(h.token(a.vault).owner, a.asset);
    assert_eq!(h.token(a.vault).delegate, COption::None);
    assert_eq!(h.token(a.vault).close_authority, COption::None);
    for (user, amount, deposit) in [
        (1, 120, true),
        (2, 80, true),
        (1, 45, false),
        (2, 80, false),
        (1, 75, false),
    ] {
        h.run(user, &[h.move_ix(&a, user, amount, deposit)])
            .unwrap();
        h.assert_conservation(&a);
    }
    assert_eq!(h.token(a.vault).amount, 0);
    let rent = h.svm.get_account(&a.positions[1]).unwrap().lamports;
    let before = h.svm.get_balance(&h.keys[1].pubkey()).unwrap();
    let meta = h.run(1, &[h.close_ix(&a, 1)]).unwrap();
    assert_eq!(
        h.svm.get_balance(&h.keys[1].pubkey()).unwrap(),
        before + rent - meta.fee
    );
    assert!(h
        .svm
        .get_account(&a.positions[1])
        .is_none_or(|a| a.lamports == 0 && a.data.is_empty()));
    h.run(1, &[h.open_ix(&a, 1)]).unwrap();
    assert_eq!(h.state::<Position>(a.positions[1]).balance, 0);
}

#[test]
fn overdraft_close_and_reinitialization_cannot_destroy_claims() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let before = h.snapshot(&a);
    custom_error(
        h.run(1, &[h.move_ix(&a, 1, 121, false)]),
        CustodyError::InsufficientBalance,
    );
    custom_error(
        h.run(1, &[h.close_ix(&a, 1)]),
        CustodyError::NonemptyPosition,
    );
    assert!(h.run(1, &[h.open_ix(&a, 1)]).is_err());
    assert!(h.run(0, &[h.register_ix(&a, 0)]).is_err());
    assert!(h.run(0, &[h.initialize_ix(0)]).is_err());
    assert_eq!(h.snapshot(&a), before);
}

#[test]
fn zero_amount_preserves_state_but_still_requires_authority() {
    let mut h = Harness::new();
    let a = h.asset();
    let before = h.snapshot(&a);
    for deposit in [true, false] {
        h.run(1, &[h.move_ix(&a, 1, 0, deposit)]).unwrap();
        let mut forged = h.move_ix(&a, 1, 0, deposit);
        forged.accounts[0].pubkey = h.keys[2].pubkey();
        assert!(h.run(2, &[forged]).is_err());
    }
    assert_eq!(h.snapshot(&a), before);
}

#[test]
fn donations_do_not_create_spendable_claims() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let donate = spl_token_interface::instruction::transfer_checked(
        &spl_token_interface::ID,
        &a.wallets[2],
        &a.mint,
        &a.vault,
        &h.keys[2].pubkey(),
        &[],
        9,
        6,
    )
    .unwrap();
    h.run(2, &[donate]).unwrap();
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 120);
    custom_error(
        h.run(2, &[h.move_ix(&a, 2, 1, false)]),
        CustodyError::InsufficientBalance,
    );
    h.run(1, &[h.move_ix(&a, 1, 120, false)]).unwrap();
    assert_eq!(h.token(a.vault).amount, 9);
    h.assert_conservation(&a);
}

#[test]
fn wrapped_sol_keeps_rent_out_of_claims_and_moves_lamports_with_tokens() {
    let mut h = Harness::new();
    let mut a = h.asset();
    a.mint = spl_token_interface::native_mint::ID;
    h.put_packed(
        a.mint,
        Mint {
            mint_authority: COption::None,
            supply: 0,
            decimals: 9,
            is_initialized: true,
            freeze_authority: COption::None,
        },
    );
    a.asset = pda(&[b"asset", h.ledger.as_ref(), a.mint.as_ref()]);
    a.vault = pda(&[b"vault", a.asset.as_ref()]);
    let rent = h.svm.minimum_balance_for_rent_exemption(TokenAccount::LEN);
    for i in 0..3 {
        let mut wallet = h.token(a.wallets[i]);
        wallet.mint = a.mint;
        wallet.is_native = COption::Some(rent);
        h.put_packed(a.wallets[i], wallet);
        let mut account = h.svm.get_account(&a.wallets[i]).unwrap();
        account.lamports = rent + INITIAL_TOKENS;
        h.svm.set_account(a.wallets[i], account).unwrap();
        a.positions[i] = pda(&[b"position", a.asset.as_ref(), h.keys[i].pubkey().as_ref()]);
    }
    h.run(0, &[h.register_ix(&a, 0)]).unwrap();
    h.run(1, &[h.open_ix(&a, 1)]).unwrap();
    assert_eq!(h.token(a.vault).is_native, COption::Some(rent));
    assert_eq!(h.token(a.vault).amount, 0);
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    assert_eq!(h.svm.get_balance(&a.vault), Some(rent + 120));
    assert_eq!(h.state::<Asset>(a.asset).total_claims, 120);
    h.run(1, &[h.move_ix(&a, 1, 120, false)]).unwrap();
    assert_eq!(h.svm.get_balance(&a.vault), Some(rent));
    assert_eq!(
        h.svm.get_balance(&a.wallets[1]),
        Some(rent + INITIAL_TOKENS)
    );
    assert_eq!(h.state::<Position>(a.positions[1]).balance, 0);
}

#[test]
fn undercollateralization_blocks_even_a_small_or_zero_withdrawal() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    // Fault injection only: no program instruction grants authority to drain a vault.
    let mut vault = h.token(a.vault);
    vault.amount -= 1;
    h.put_packed(a.vault, vault);
    let before = h.snapshot(&a);
    for amount in [0, 1, 45] {
        custom_error(
            h.run(1, &[h.move_ix(&a, 1, amount, false)]),
            CustodyError::Undercollateralized,
        );
        assert_eq!(h.snapshot(&a), before);
    }
}

#[test]
fn rejects_cross_mint_cross_ledger_wrong_vault_owner_and_missing_signature() {
    let mut h = Harness::new();
    let a = h.asset();
    let b = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let old_ledger = h.ledger;
    h.run(0, &[h.initialize_ix(1)]).unwrap();
    h.ledger = pda(&[b"ledger", h.keys[0].pubkey().as_ref(), &1u64.to_le_bytes()]);
    let mut c = a.clone();
    c.asset = pda(&[b"asset", h.ledger.as_ref(), a.mint.as_ref()]);
    c.vault = pda(&[b"vault", c.asset.as_ref()]);
    h.run(0, &[h.register_ix(&c, 0)]).unwrap();
    h.ledger = old_ledger;
    let before = h.snapshot(&a);
    for deposit in [true, false] {
        // Actual initialized alternatives, not missing accounts that fail to load.
        for (index, replacement) in [
            (1, c.asset),
            (2, a.positions[2]),
            (3, b.mint),
            (4, b.vault),
            (5, a.wallets[2]),
            (6, TOKEN_2022),
        ] {
            let mut forged = h.move_ix(&a, 1, 1, deposit);
            forged.accounts[index].pubkey = replacement;
            assert!(
                h.run(1, &[forged]).is_err(),
                "accepted substitution at {index}"
            );
            assert_eq!(h.snapshot(&a), before);
        }
        let mut no_signer = h.move_ix(&a, 1, 1, deposit);
        no_signer.accounts[0].is_signer = false;
        let e = h.run(2, &[no_signer]).unwrap_err();
        assert!(
            e.meta.logs.iter().any(|s| s.contains("AccountNotSigner")),
            "{:?}",
            e
        );
        let mut alias = h.move_ix(&a, 1, 0, deposit);
        alias.accounts[5].pubkey = a.vault;
        assert!(h.run(1, &[alias]).is_err());
    }
    assert_eq!(h.snapshot(&a), before);
    // Even the ledger administrator cannot spend another holder's position.
    let mut admin = h.move_ix(&a, 1, 1, false);
    admin.accounts[0].pubkey = h.keys[0].pubkey();
    assert!(h.run(0, &[admin]).is_err());
}

#[test]
fn registration_requires_ledger_authority_and_matching_token_program() {
    let mut h = Harness::new();
    let a = h.asset();
    // An unused asset avoids an earlier duplicate-initialization failure masking
    // the authority check. The mint is an actual initialized SPL mint.
    let mut new_asset = a.clone();
    new_asset.mint = h.address();
    let mint = Mint::unpack(&h.svm.get_account(&a.mint).unwrap().data).unwrap();
    h.put_packed(new_asset.mint, mint);
    new_asset.asset = pda(&[b"asset", h.ledger.as_ref(), new_asset.mint.as_ref()]);
    new_asset.vault = pda(&[b"vault", new_asset.asset.as_ref()]);
    let err = h.run(1, &[h.register_ix(&new_asset, 1)]).unwrap_err();
    assert!(
        err.meta.logs.iter().any(|s| s.contains("ConstraintHasOne")),
        "{:?}",
        err
    );
    assert!(h.svm.get_account(&new_asset.asset).is_none());
    assert!(h.svm.get_account(&new_asset.vault).is_none());
    // Keep the asset uninitialized so duplicate initialization cannot mask
    // the mint/token-program mismatch.
    let mut mint_account = h.svm.get_account(&new_asset.mint).unwrap();
    mint_account.owner = TOKEN_2022;
    h.svm.set_account(new_asset.mint, mint_account).unwrap();
    assert!(h.run(0, &[h.register_ix(&new_asset, 0)]).is_err());
    assert!(h.svm.get_account(&new_asset.asset).is_none());
    assert!(h.svm.get_account(&new_asset.vault).is_none());
    let mut mint_account = h.svm.get_account(&a.mint).unwrap();
    mint_account.owner = TOKEN_2022;
    h.svm.set_account(a.mint, mint_account).unwrap();
    assert!(h.run(1, &[h.move_ix(&a, 1, 1, true)]).is_err());
}

#[test]
fn invalid_program_owner_and_layout_version_are_rejected() {
    let mut h = Harness::new();
    let a = h.asset();
    let original = h.svm.get_account(&a.positions[1]).unwrap();
    let mut forged = original.clone();
    forged.owner = SYSTEM;
    h.svm.set_account(a.positions[1], forged).unwrap();
    let e = h.run(1, &[h.move_ix(&a, 1, 1, true)]).unwrap_err();
    assert!(e
        .meta
        .logs
        .iter()
        .any(|s| s.contains("AccountOwnedByWrongProgram")));
    h.svm.set_account(a.positions[1], original).unwrap();
    let mut account = h.svm.get_account(&a.asset).unwrap();
    let mut state = h.state::<Asset>(a.asset);
    state.version = 255;
    state
        .try_serialize(&mut account.data.as_mut_slice())
        .unwrap();
    h.svm.set_account(a.asset, account).unwrap();
    custom_error(
        h.run(1, &[h.move_ix(&a, 1, 1, true)]),
        CustodyError::UnsupportedVersion,
    );
}

#[test]
fn token_cpi_failure_rolls_back_both_sides_of_the_ledger() {
    let mut h = Harness::new();
    let a = h.asset();
    h.run(1, &[h.move_ix(&a, 1, 120, true)]).unwrap();
    let freeze = spl_token_interface::instruction::freeze_account(
        &spl_token_interface::ID,
        &a.wallets[1],
        &a.mint,
        &h.keys[0].pubkey(),
        &[],
    )
    .unwrap();
    h.run(0, &[freeze]).unwrap();
    let before = h.snapshot(&a);
    for deposit in [false, true] {
        let err = h.run(1, &[h.move_ix(&a, 1, 45, deposit)]).unwrap_err();
        assert!(
            err.meta.logs.iter().any(|s| s.contains("frozen")),
            "{:?}",
            err
        );
        assert_eq!(h.snapshot(&a), before);
    }
}

#[test]
fn late_failure_reverts_every_asset_in_an_atomic_basket() {
    let mut h = Harness::new();
    let a = h.asset();
    let b = h.asset();
    let before_a = h.snapshot(&a);
    let before_b = h.snapshot(&b);
    let err = h
        .run(
            1,
            &[
                h.move_ix(&a, 1, 120, true),
                h.move_ix(&b, 1, INITIAL_TOKENS + 1, true),
            ],
        )
        .unwrap_err();
    assert!(matches!(err.err, TransactionError::InstructionError(1, _)));
    assert_eq!(h.snapshot(&a), before_a);
    assert_eq!(h.snapshot(&b), before_b);
    h.run(
        1,
        &[h.move_ix(&a, 1, 120, true), h.move_ix(&b, 1, 120, true)],
    )
    .unwrap();
    let before_a = h.snapshot(&a);
    let before_b = h.snapshot(&b);
    let err = h
        .run(
            1,
            &[h.move_ix(&a, 1, 45, false), h.move_ix(&b, 1, 121, false)],
        )
        .unwrap_err();
    assert_eq!(
        err.err,
        TransactionError::InstructionError(
            1,
            InstructionError::Custom(6000 + CustodyError::InsufficientBalance as u32)
        )
    );
    assert_eq!(h.snapshot(&a), before_a);
    assert_eq!(h.snapshot(&b), before_b);
}

#[test]
fn u64_boundary_and_corrupted_aggregate_fail_without_wrapping() {
    let mut h = Harness::new();
    let a = h.asset();
    for i in 0..3 {
        let mut wallet = h.token(a.wallets[i]);
        wallet.amount = if i == 1 { u64::MAX } else { 0 };
        h.put_packed(a.wallets[i], wallet);
    }
    let mut mint = Mint::unpack(&h.svm.get_account(&a.mint).unwrap().data).unwrap();
    mint.supply = u64::MAX;
    h.put_packed(a.mint, mint);
    h.run(1, &[h.move_ix(&a, 1, u64::MAX, true)]).unwrap();
    h.assert_conservation(&a);
    let before = h.snapshot(&a);
    custom_error(
        h.run(1, &[h.move_ix(&a, 1, 1, true)]),
        CustodyError::ArithmeticOverflow,
    );
    custom_error(
        h.run(2, &[h.move_ix(&a, 2, 1, true)]),
        CustodyError::ArithmeticOverflow,
    );
    assert_eq!(h.snapshot(&a), before);
    h.run(1, &[h.move_ix(&a, 1, u64::MAX, false)]).unwrap();
    h.assert_conservation(&a);
    h.run(1, &[h.move_ix(&a, 1, 10, true)]).unwrap();
    let mut account = h.svm.get_account(&a.asset).unwrap();
    let mut state = h.state::<Asset>(a.asset);
    state.total_claims = 0;
    state
        .try_serialize(&mut account.data.as_mut_slice())
        .unwrap();
    h.svm.set_account(a.asset, account).unwrap();
    let before = h.snapshot(&a);
    custom_error(
        h.run(1, &[h.move_ix(&a, 1, 1, false)]),
        CustodyError::ArithmeticOverflow,
    );
    assert_eq!(h.snapshot(&a), before);
}

#[test]
fn unrelated_assets_have_disjoint_protocol_write_sets() {
    let mut h = Harness::new();
    let a = h.asset();
    let b = h.asset();
    let writes = |ix: Instruction| -> BTreeSet<Address> {
        ix.accounts
            .iter()
            .filter(|m| m.is_writable)
            .map(|m| m.pubkey)
            .collect()
    };
    let wa = writes(h.move_ix(&a, 1, 1, true));
    let wb = writes(h.move_ix(&b, 2, 1, true));
    assert_eq!(wa.len(), 4);
    assert!(wa.is_disjoint(&wb));
    assert!(!wa.contains(&h.ledger));
    assert!(!wb.contains(&h.ledger));
}

#[test]
fn measure_atomic_custody_baskets() {
    let mut h = Harness::new();
    let assets: Vec<_> = (0..7).map(|_| h.asset()).collect();
    let mut rows = Vec::new();
    for count in [1, 2, 4, 5, 6, 7] {
        for deposit in [true, false] {
            let mut instructions = vec![ComputeBudgetInstruction::set_compute_unit_limit(200_000)];
            instructions.extend(assets[..count].iter().map(|a| h.move_ix(a, 1, 10, deposit)));
            h.svm.expire_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &instructions,
                Some(&h.keys[1].pubkey()),
                &[&h.keys[1]],
                h.svm.latest_blockhash(),
            );
            let bytes = wincode::serialize(&tx).unwrap().len();
            let accounts = tx.message.account_keys.len();
            let writable = tx
                .message
                .account_keys
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    tx.message.is_maybe_writable_with_reserved_addresses(
                        *i,
                        None::<&std::collections::HashSet<Address>>,
                    )
                })
                .count();
            let loaded_bytes: usize = tx
                .message
                .account_keys
                .iter()
                .filter_map(|k| h.svm.get_account(k))
                .map(|a| a.data.len())
                .sum();
            // LiteSVM is a runtime, not the networking layer. Enforce the actual
            // wire packet limit here before execution, without disabling checks.
            let cu = if bytes <= PACKET_BYTES {
                Some(h.svm.send_transaction(tx).unwrap().compute_units_consumed)
            } else {
                None
            };
            if count <= 5 {
                assert!(cu.is_some(), "baseline basket no longer fits");
            }
            if let Some(cu) = cu {
                assert!(cu <= 200_000);
            }
            rows.push(serde_json::json!({"assets":count,"operation":if deposit {"deposit"} else {"withdraw"},
                "legacy_transaction_bytes":bytes,"account_keys":accounts,"writable_accounts":writable,
                "account_data_bytes":loaded_bytes,"compute_units":cu,"fits_packet":bytes <= PACKET_BYTES}));
        }
    }
    let report = serde_json::to_string_pretty(&rows).unwrap();
    println!("CUSTODY_BASKET_COSTS\n{report}");
    if let Some(path) = std::env::var_os(COST_REPORT_ENV) {
        std::fs::write(path, format!("{report}\n")).unwrap();
    }
}
