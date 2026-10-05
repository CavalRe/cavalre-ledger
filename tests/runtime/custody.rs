//! Replay unchanged Solidity observations through Ledger and both token programs.
use super::*;

#[test]
fn replays_all_original_solidity_custody_steps_with_classic_and_token2022() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../spec/fixtures/custody.json")).unwrap();
    assert_eq!(fixture["schema_version"], 1);
    assert_eq!(
        fixture["contracts_commit"],
        "34d159ff4e88fdfdee16738d9a1228f0bf407212"
    );
    let initial = fixture["initial_tokens_per_user"].as_u64().unwrap();
    let steps = fixture["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 138);
    for token_program in [TOKEN, TOKEN_2022] {
        let mut h = Harness::new();
        let mut token = External::setup(&mut h, 174, 1, token_program);
        let mut mint = Mint::unpack(&h.svm.get_account(&token.mint).unwrap().data).unwrap();
        mint.supply = 2 * initial;
        h.pack_for(token.mint, mint, token_program);
        let users = [h.key(1), h.key(2)];
        let wallets = [token.wallet, Address::new_from_array([176; 32])];
        for (wallet, owner) in wallets.iter().zip(users) {
            h.pack_for(
                *wallet,
                TokenAccount {
                    mint: token.mint,
                    owner,
                    amount: initial,
                    delegate: COption::None,
                    state: AccountState::Initialized,
                    is_native: COption::None,
                    delegated_amount: 0,
                    close_authority: COption::None,
                },
                token_program,
            );
        }
        let registration = token.registration(&h);
        succeeds(&mut h, &[0], registration);
        let positions = users.map(|user| child(token.root, user));
        let mut failures = 0;
        for (index, step) in steps.iter().enumerate() {
            let user = step["user"].as_u64().unwrap() as usize;
            let kind = step["kind"].as_u64().unwrap();
            let amount = step["amount"].as_u64().unwrap();
            token.wallet = wallets[user];
            let instruction = match kind {
                0 | 1 => token.movement(
                    &h,
                    (users[user], user + 1),
                    (token.root, users[user]),
                    amount,
                    kind == 0,
                    &[],
                ),
                2 => spl_token_2022_interface::instruction::transfer_checked(
                    &ap(token_program),
                    &ap(wallets[user]),
                    &ap(token.mint),
                    &ap(token.vault),
                    &ap(users[user]),
                    &[],
                    amount,
                    6,
                )
                .unwrap(),
                _ => panic!("unknown custody fixture kind {kind}"),
            };
            let ledger_keys = [token.root_storage, token.source, positions[0], positions[1]];
            let before = ledger_keys.map(|key| h.svm.get_account(&key));
            if step["success"].as_bool().unwrap() {
                let result = run(&mut h, &[0, user + 1], instruction);
                assert!(result.is_ok(), "{token_program}, step {index}: {result:?}");
            } else {
                failures += 1;
                let error = if kind == 0 {
                    spl_token_interface::error::TokenError::InsufficientFunds as u32
                } else {
                    assert_eq!(kind, 1);
                    LedgerError::Accounting.into()
                };
                // Checks the exact rejection and every supplied account, allowing
                // only the independent transaction payer's fee to change.
                rejects(&mut h, &[0, user + 1], instruction, error);
            }
            if kind == 2 || !step["success"].as_bool().unwrap() {
                assert_eq!(
                    ledger_keys.map(|key| h.svm.get_account(&key)),
                    before,
                    "{token_program}, step {index}: changed Ledger state"
                );
            }
            for i in 0..2 {
                let position = h
                    .svm
                    .get_account(&positions[i])
                    .map(|_| h.record(positions[i]));
                assert_eq!(
                    position.as_ref().map_or(0, |a| a.debit),
                    u128::from(step["positions"][i].as_u64().unwrap()),
                    "{token_program}, step {index}, position {i}"
                );
                assert_eq!(position.as_ref().map_or(0, |a| a.credit), 0);
                assert!(
                    !position.is_some_and(|a| a.registered),
                    "receipt must remain implicit"
                );
                assert_eq!(
                    h.token(wallets[i]),
                    step["wallets"][i].as_u64().unwrap(),
                    "{token_program}, step {index}, wallet {i}"
                );
            }
            let claims = u128::from(step["total_claims"].as_u64().unwrap());
            let root = h.record(token.root);
            let source = h.record(token.source);
            assert_eq!(
                (root.debit, root.credit),
                (claims, claims),
                "{token_program}, step {index}, root"
            );
            assert_eq!(
                (source.debit, source.credit),
                (0, claims),
                "{token_program}, step {index}, Source"
            );
            assert_eq!(
                h.token(token.vault),
                step["vault"].as_u64().unwrap(),
                "{token_program}, step {index}, vault"
            );
            assert!(u128::from(h.token(token.vault)) >= claims);
            assert_eq!(
                h.token(token.vault) + wallets.map(|w| h.token(w)).iter().sum::<u64>(),
                2 * initial
            );
        }
        assert_eq!(failures, 13);
    }
}
