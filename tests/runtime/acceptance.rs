//! Standalone product acceptance cases executed against the actual sBPF program.
use super::*;
use anchor_lang::error::ErrorCode;
use ledger::ledger::LedgerError;
use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use solana_instruction_error::InstructionError;
use solana_transaction_error::TransactionError;
#[path = "backing.rs"]
mod backing;
#[path = "children.rs"]
mod children;
#[path = "custody.rs"]
mod custody;
#[path = "events.rs"]
mod events;
#[path = "execution_limits.rs"]
mod execution_limits;
#[path = "identities.rs"]
mod identities;
#[path = "metadata.rs"]
mod metadata;
#[path = "names.rs"]
mod names;
#[path = "native_sol.rs"]
mod native_sol;
#[path = "root.rs"]
mod root;
#[path = "token2022.rs"]
mod token2022;
#[path = "writable_accounts.rs"]
mod writable_accounts;

fn run(
    h: &mut Harness,
    signers: &[usize],
    instruction: Instruction,
) -> Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
    run_raw(h, signers, h.indexed(instruction))
}

fn run_raw(
    h: &mut Harness,
    signers: &[usize],
    instruction: Instruction,
) -> Result<TransactionMetadata, Box<FailedTransactionMetadata>> {
    h.svm.expire_blockhash();
    let keys: Vec<_> = signers.iter().map(|i| &h.keys[*i]).collect();
    let tx = Transaction::new_signed_with_payer(
        &[instruction],
        Some(&h.key(0)),
        &keys,
        h.svm.latest_blockhash(),
    );
    h.svm.send_transaction(tx).map_err(Box::new)
}

fn succeeds(h: &mut Harness, signers: &[usize], instruction: Instruction) {
    if let Err(error) = run(h, signers, instruction) {
        panic!("unexpected failure: {error:?}");
    }
}

// Assert the intended rejection, not just any transaction failure. Compare all
// supplied accounts, including unallocated destinations and token accounts.
// Only the transaction payer's fee is excluded; its data/owner are still checked.
fn rejects(h: &mut Harness, signers: &[usize], instruction: Instruction, code: u32) {
    rejects_with_error(h, signers, instruction, InstructionError::Custom(code));
}

fn rejects_with_error(
    h: &mut Harness,
    signers: &[usize],
    instruction: Instruction,
    error: InstructionError,
) {
    let instruction = h.indexed(instruction);
    let before: Vec<_> = instruction
        .accounts
        .iter()
        .map(|m| (m.pubkey, h.svm.get_account(&m.pubkey)))
        .collect();
    let failure = run_raw(h, signers, instruction).expect_err("expected rejection");
    assert_eq!(
        failure.err,
        TransactionError::InstructionError(0, error),
        "wrong rejection: {failure:?}"
    );
    for (key, old) in before {
        let mut new = h.svm.get_account(&key);
        if key == h.key(0) {
            if let (Some(old), Some(new)) = (&old, &mut new) {
                assert_eq!(
                    new.lamports.checked_add(failure.meta.fee),
                    Some(old.lamports),
                    "rejected transaction charged more than its fee"
                );
                new.lamports = old.lamports;
            }
        }
        assert_eq!(new, old, "rejected transaction changed {key}");
    }
}

fn remaining(root: Address, keys: &[Address]) -> Vec<Address> {
    let mut result = Vec::new();
    for key in keys {
        if *key != root && !result.contains(key) {
            result.push(*key);
        }
    }
    result
}

struct External {
    token_program: Address,
    mint: Address,
    wallet: Address,
    root: Address,
    root_storage: Address,
    source: Address,
    vault: Address,
}
impl External {
    fn new(h: &mut Harness, tag: u8, owner: usize) -> Self {
        Self::named(h, tag, owner, "Token")
    }
    fn named(h: &mut Harness, tag: u8, owner: usize, name: &str) -> Self {
        let token = Self::setup(h, tag, owner, TOKEN);
        h.metadata(token.mint, name, "TOK");
        succeeds(h, &[0], token.registration(h));
        token
    }
    // Native mint/wallet fixtures; Ledger registration and settlement execute sBPF.
    fn setup(h: &mut Harness, tag: u8, owner: usize, token_program: Address) -> Self {
        let mint = Address::new_from_array([tag; 32]);
        let wallet = Address::new_from_array([tag + 1; 32]);
        h.pack_for(
            mint,
            Mint {
                mint_authority: COption::None,
                supply: 1000,
                decimals: 6,
                is_initialized: true,
                freeze_authority: COption::None,
            },
            token_program,
        );
        h.pack_for(
            wallet,
            TokenAccount {
                mint,
                owner: h.key(owner),
                amount: 1000,
                delegate: COption::None,
                state: AccountState::Initialized,
                is_native: COption::None,
                delegated_amount: 0,
                close_authority: COption::None,
            },
            token_program,
        );
        h.metadata(mint, "Token", "TOK");
        let root = h.external_root(mint);
        let root_storage = h.storage(root);
        let source = child(root, sa(SOURCE));
        let vault = sa(anchor_lang::prelude::Pubkey::find_program_address(
            &[b"vault", root_storage.as_ref()],
            &ledger::ID,
        )
        .0);
        Self {
            token_program,
            mint,
            wallet,
            root,
            root_storage,
            source,
            vault,
        }
    }
    fn registration(&self, h: &Harness) -> Instruction {
        let mut instruction = ix(
            accounts::RegisterToken {
                global_root: ledger::ledger_lib::global_root_address().0,
                payer: ap(h.key(0)),
                root: ap(self.root_storage),
                mint: ap(self.mint),
                vault: ap(self.vault),
                token_program: ap(self.token_program),
                system_program: ap(SYSTEM),
            },
            instruction::AddExternalToken {},
            &[self.source],
        );
        instruction.accounts.push(AccountMeta::new_readonly(
            sa(ledger::ledger_view::metadata_address(&ap(self.mint))),
            false,
        ));
        instruction
    }

    fn movement(
        &self,
        h: &Harness,
        authorization: (Address, usize),
        endpoint: (Address, Address),
        amount: u64,
        deposit: bool,
        extra: &[Address],
    ) -> Instruction {
        let (authority, funder) = authorization;
        let (parent, relative) = endpoint;
        let accounts = accounts::MoveTokens {
            payer: ap(h.key(0)),
            authority: ap(authority),
            funding_authority: ap(h.key(funder)),
            root: ap(self.root_storage),
            mint: ap(self.mint),
            vault: ap(self.vault),
            wallet: ap(self.wallet),
            token_program: ap(self.token_program),
            system_program: ap(SYSTEM),
        };
        let mut rest = vec![self.source, parent, child(parent, relative)];
        rest.extend(extra);
        let rest = remaining(self.root, &rest);
        if deposit {
            ix(
                accounts,
                instruction::Wrap {
                    parent: ap(parent),
                    relative: ap(relative),
                    amount,
                },
                &rest,
            )
        } else {
            ix(
                accounts,
                instruction::Unwrap {
                    parent: ap(parent),
                    relative: ap(relative),
                    amount,
                },
                &rest,
            )
        }
    }
}

fn base(h: &Harness, root: Address, authority: Address) -> accounts::LedgerAccounts {
    accounts::LedgerAccounts {
        payer: ap(h.key(0)),
        authority: ap(authority),
        root: ap(h.storage(root)),
        system_program: ap(SYSTEM),
    }
}
fn group(
    h: &Harness,
    root: Address,
    authority: Address,
    parent: Address,
    relative: Address,
    implicit_allowed: bool,
    extra: &[Address],
) -> Instruction {
    let mut rest = vec![parent, child(parent, relative)];
    rest.extend(extra);
    ix(
        base(h, root, authority),
        instruction::AddSubAccountGroup {
            parent: ap(parent),
            relative: ap(relative),
            name: "Group".into(),
            credit: false,
            implicit_allowed,
        },
        &remaining(root, &rest),
    )
}
fn leaf(
    h: &Harness,
    root: Address,
    authority: Address,
    parent: Address,
    relative: Address,
    name: &str,
    credit: bool,
) -> Instruction {
    ix(
        base(h, root, authority),
        instruction::AddSubAccount {
            parent: ap(parent),
            relative: ap(relative),
            name: name.into(),
            credit,
        },
        &remaining(root, &[parent, child(parent, relative)]),
    )
}
fn remove(
    h: &Harness,
    root: Address,
    authority: Address,
    parent: Address,
    relative: Address,
    is_group: bool,
) -> Instruction {
    let rest = remaining(root, &[parent, child(parent, relative)]);
    if is_group {
        ix(
            base(h, root, authority),
            instruction::RemoveSubAccountGroup {
                parent: ap(parent),
                relative: ap(relative),
            },
            &rest,
        )
    } else {
        ix(
            base(h, root, authority),
            instruction::RemoveSubAccount {
                parent: ap(parent),
                relative: ap(relative),
            },
            &rest,
        )
    }
}
fn transfer(
    h: &Harness,
    root: Address,
    authority: Address,
    from: (Address, Address),
    to: (Address, Address),
    amount: u128,
    extra: &[Address],
) -> Instruction {
    let mut rest = vec![from.0, child(from.0, from.1), to.0, child(to.0, to.1)];
    rest.extend(extra);
    ix(
        base(h, root, authority),
        instruction::Transfer {
            from_parent: ap(from.0),
            from: ap(from.1),
            to_parent: ap(to.0),
            to: ap(to.1),
            amount,
        },
        &remaining(root, &rest),
    )
}
fn branch(h: &mut Harness, root: Address, owner: usize, implicit: bool) -> Address {
    let authority = h.key(owner);
    let create = group(h, root, authority, root, authority, implicit, &[]);
    let signers = if owner == 0 { vec![0] } else { vec![0, owner] };
    succeeds(h, &signers, create);
    child(root, authority)
}
fn proxy(h: &Harness, app: Address, authority: Address, mut inner: Instruction) -> Instruction {
    for meta in &mut inner.accounts {
        if meta.pubkey == authority {
            meta.is_signer = false;
        }
    }
    let mut accounts = vec![
        AccountMeta::new_readonly(h.key(0), true),
        AccountMeta::new_readonly(sa(ledger::ID), false),
    ];
    accounts.extend(inner.accounts);
    Instruction {
        program_id: app,
        accounts,
        data: inner.data,
    }
}

fn custody_proxy(h: &Harness, app: Address, authority: Address, inner: Instruction) -> Instruction {
    use anchor_lang::Discriminator;
    let mut call = proxy(h, app, authority, inner);
    if [
        instruction::Wrap::DISCRIMINATOR,
        instruction::Unwrap::DISCRIMINATOR,
        instruction::WrapSol::DISCRIMINATOR,
        instruction::UnwrapSol::DISCRIMINATOR,
    ]
    .iter()
    .any(|d| call.data.starts_with(d))
    {
        call.data.splice(..0, b"custody-helper".iter().copied());
    }
    call
}

#[test]
fn distinct_payer_funds_application_pda_and_unsigned_recipient_can_withdraw() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 1);
    let app = Address::new_from_array([62; 32]);
    let binary = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/deploy/cavalre_ledger_test_consumer.so"),
    )
    .unwrap();
    h.svm.add_program(app, &binary).unwrap();
    let authority = sa(anchor_lang::prelude::Pubkey::find_program_address(
        &[b"app", h.key(0).as_ref()],
        &ap(app),
    )
    .0);
    let b = child(e.root, authority);
    let receiver = h.key(2);
    let i = proxy(
        &h,
        app,
        authority,
        group(&h, e.root, authority, e.root, authority, true, &[]),
    );
    succeeds(&mut h, &[0], i);

    // A generic SPL delegation does not authorize Ledger to pull another owner's tokens.
    let mut wallet = TokenAccount::unpack(&h.svm.get_account(&e.wallet).unwrap().data).unwrap();
    wallet.delegate = COption::Some(h.key(0));
    wallet.delegated_amount = 1000;
    h.pack(e.wallet, wallet);
    let i = proxy(
        &h,
        app,
        authority,
        e.movement(&h, (authority, 0), (b, receiver), 100, true, &[]),
    );
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());

    // Strip the payer signature to reach the program's Signer validation.
    let mut i = proxy(
        &h,
        app,
        authority,
        e.movement(&h, (authority, 1), (b, receiver), 100, true, &[]),
    );
    for meta in &mut i.accounts {
        if meta.pubkey == h.key(1) {
            meta.is_signer = false;
        }
    }
    rejects(&mut h, &[0], i, ErrorCode::AccountNotSigner.into());
    let i = proxy(
        &h,
        app,
        authority,
        e.movement(&h, (authority, 1), (b, receiver), 100, true, &[]),
    );
    succeeds(&mut h, &[0, 1], i);
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (900, 100));
    assert_eq!(h.record(child(b, receiver)).debit, 100);
    assert!(!h.record(child(b, receiver)).registered);

    // The application's administrator cannot substitute for the application's PDA.
    let i = e.movement(&h, (h.key(0), 0), (b, receiver), 40, false, &[]);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = proxy(
        &h,
        app,
        authority,
        e.movement(&h, (authority, 0), (b, receiver), 40, false, &[]),
    );
    succeeds(&mut h, &[0], i); // Wallet owner 1 and ledger recipient 2 do not sign.
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (940, 60));
    assert_eq!((h.record(e.root).debit, h.record(e.root).credit), (60, 60));
    assert_eq!(h.record(e.source).credit, 60);
}

#[test]
fn tree_authority_cannot_capture_other_branches_or_reserved_accounts() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let b = branch(&mut h, e.root, 1, true);
    let i = group(&h, e.root, h.key(0), e.root, h.key(1), true, &[]);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = leaf(&h, e.root, h.key(0), b, h.key(2), "Capture", false);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = remove(&h, e.root, h.key(0), e.root, h.key(1), true);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = leaf(&h, e.root, h.key(0), a, h.key(2), "Credit", true);
    rejects(&mut h, &[0], i, LedgerError::InvalidKind.into());
    let i = ix(
        base(&h, e.root, h.key(0)),
        instruction::AddSubAccountGroup {
            parent: ap(a),
            relative: ap(h.key(2)),
            name: "Credit".into(),
            credit: true,
            implicit_allowed: true,
        },
        &[a, child(a, h.key(2))],
    );
    rejects(&mut h, &[0], i, LedgerError::InvalidKind.into());
    let i = leaf(
        &h,
        e.root,
        h.key(0),
        e.root,
        sa(SOURCE),
        "Capture Source",
        true,
    );
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = remove(&h, e.root, h.key(0), e.root, sa(SOURCE), false);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    // Root ownership does not permit internal-ledger Source metadata mutation either.
    let (root, _) = h.internal();
    let i = remove(&h, root, h.key(0), root, sa(SOURCE), false);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
}

#[test]
fn transfers_require_same_custodian_leaf_kind_membership_and_self_transfer_funds() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let b = branch(&mut h, e.root, 1, true);
    let user = h.key(2);
    let i = e.movement(&h, (h.key(0), 0), (a, user), 100, true, &[]);
    succeeds(&mut h, &[0], i);
    for destination in [(b, user), (e.root, sa(SOURCE)), (e.root, h.key(0))] {
        let i = transfer(&h, e.root, h.key(0), (a, user), destination, 1, &[]);
        rejects(&mut h, &[0], i, LedgerError::Accounting.into());
    }
    let i = transfer(&h, e.root, h.key(1), (a, user), (a, user), 1, &[]);
    rejects(&mut h, &[0, 1], i, LedgerError::Accounting.into());
    let i = transfer(&h, e.root, h.key(0), (a, user), (a, user), 101, &[]);
    rejects(&mut h, &[0], i, LedgerError::Accounting.into());
    let before = h.svm.get_account(&child(a, user)).unwrap();
    let i = transfer(&h, e.root, h.key(0), (a, user), (a, user), 100, &[]);
    succeeds(&mut h, &[0], i);
    assert_eq!(h.svm.get_account(&child(a, user)).unwrap(), before);
    let other = External::new(&mut h, 70, 0);
    let i = transfer(&h, e.root, h.key(0), (a, user), (other.root, user), 1, &[]);
    rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
}

#[test]
fn registered_leaf_labels_allow_empty_names_and_keep_the_byte_limit() {
    let mut h = Harness::new();
    let (root, source) = h.internal();
    let relative = h.key(1);
    let account = child(root, relative);
    let fund = transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (root, relative),
        17,
        &[],
    );
    succeeds(&mut h, &[0], fund);
    let register = leaf(&h, root, h.key(0), root, relative, "", false);
    succeeds(&mut h, &[0], register.clone());
    let record = h.record(account);
    assert!(record.registered);
    assert!(record.name.is_empty());
    assert_eq!((record.kind, record.debit, record.sub_index), (2, 17, 2));
    let mut reader = ledger::ledger_view::Reader::new();
    for key in [root, account] {
        let a = h.svm.get_account(&key).unwrap();
        reader.insert(ap(key), &ap(a.owner), &a.data).unwrap();
    }
    let view = reader
        .account_view(&ap(root), &ap(root), &ap(relative))
        .unwrap();
    assert!(view.registered);
    assert!(view.name.is_empty());
    assert_eq!(view.balances.debit, 17);
    // Matching empty-label registration retains funded state, index and rent.
    let before = [root, source, account].map(|key| h.svm.get_account(&key));
    let payer_before = h.svm.get_account(&h.key(0)).unwrap().lamports;
    let repeat = run(&mut h, &[0], register).unwrap();
    assert!(events::event_bytes(&repeat.logs).is_empty());
    assert_eq!(
        h.svm.get_account(&h.key(0)).unwrap().lamports + repeat.fee,
        payer_before
    );
    assert_eq!(
        [root, source, account].map(|key| h.svm.get_account(&key)),
        before
    );
    let conflict = leaf(&h, root, h.key(0), root, relative, "Changed", false);
    rejects(&mut h, &[0], conflict, LedgerError::MetadataConflict.into());
    let credit_relative = h.key(2);
    let credit = leaf(&h, root, h.key(0), root, credit_relative, "", true);
    succeeds(&mut h, &[0], credit);
    assert_eq!(h.record(child(root, credit_relative)).kind, 3);
    // Names are bounded in UTF-8 bytes, not characters.
    let long_relative = Address::new_from_array([231; 32]);
    let max_name = "é".repeat(32);
    let named = leaf(&h, root, h.key(0), root, long_relative, &max_name, false);
    succeeds(&mut h, &[0], named);
    assert_eq!(h.record(child(root, long_relative)).name, max_name);
    let invalid_relative = Address::new_from_array([232; 32]);
    for name in ["N".repeat(65), "é".repeat(33)] {
        let invalid = leaf(&h, root, h.key(0), root, invalid_relative, &name, false);
        rejects(&mut h, &[0], invalid, LedgerError::InvalidName.into());
    }
    for name in [String::new(), "N".repeat(65)] {
        let invalid = ix(
            base(&h, root, h.key(0)),
            instruction::AddSubAccountGroup {
                parent: ap(root),
                relative: ap(invalid_relative),
                name,
                credit: false,
                implicit_allowed: true,
            },
            &[child(root, invalid_relative)],
        );
        rejects(&mut h, &[0], invalid, LedgerError::InvalidName.into());
    }
}

#[test]
fn registration_preserves_funded_implicit_balances_and_checks_repeated_calls() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let user = h.key(2);
    let account = child(a, user);
    let i = e.movement(&h, (h.key(0), 0), (a, user), 100, true, &[]);
    succeeds(&mut h, &[0], i);
    // Removing an implicit balance is an authorized no-op, not deletion.
    let before = h.svm.get_account(&account).unwrap();
    let i = remove(&h, e.root, h.key(0), a, user, false);
    succeeds(&mut h, &[0], i);
    assert_eq!(h.svm.get_account(&account).unwrap(), before);
    let i = remove(&h, e.root, h.key(1), a, user, false);
    rejects(&mut h, &[0, 1], i, LedgerError::Unauthorized.into());
    let i = group(&h, e.root, h.key(0), a, user, true, &[]);
    rejects(&mut h, &[0], i, LedgerError::Nonempty.into());
    let create = leaf(&h, e.root, h.key(0), a, user, "User", false);
    succeeds(&mut h, &[0], create.clone());
    assert_eq!(h.record(a).children, 1);
    assert_eq!(h.record(account).debit, 100);
    let registered = h.svm.get_account(&account).unwrap();
    succeeds(&mut h, &[0], create);
    assert_eq!(h.svm.get_account(&account).unwrap(), registered);
    assert_eq!(h.record(a).children, 1);
    let i = leaf(&h, e.root, h.key(0), a, user, "Rename", false);
    rejects(&mut h, &[0], i, LedgerError::MetadataConflict.into());
    let i = leaf(&h, e.root, h.key(1), a, user, "User", false);
    rejects(&mut h, &[0, 1], i, LedgerError::Unauthorized.into());
    let i = group(&h, e.root, h.key(0), e.root, h.key(0), false, &[]);
    rejects(&mut h, &[0], i, LedgerError::MetadataConflict.into());
    let i = remove(&h, e.root, h.key(0), a, user, false);
    rejects(&mut h, &[0], i, LedgerError::Nonempty.into());
    let i = e.movement(&h, (h.key(0), 0), (a, user), 100, false, &[]);
    succeeds(&mut h, &[0], i);
    let i = remove(&h, e.root, h.key(0), a, user, false);
    succeeds(&mut h, &[0], i);
    assert!(!h.record(account).registered);
    assert_eq!(h.record(a).children, 0);
    let i = remove(&h, e.root, h.key(0), e.root, h.key(0), true);
    succeeds(&mut h, &[0], i);
    assert!(!h.record(a).registered);
    assert_eq!(h.record(e.root).children, 1); // Source remains.
}

#[test]
fn registered_only_parent_checks_every_monetary_route_and_allows_subgroup_policy() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, false);
    let user = h.key(1);
    let implicit_user = h.key(2);
    let i = leaf(&h, e.root, h.key(0), a, user, "Registered", false);
    succeeds(&mut h, &[0], i);
    let i = e.movement(&h, (h.key(0), 0), (a, user), 100, true, &[]);
    succeeds(&mut h, &[0], i);
    for deposit in [true, false] {
        let i = e.movement(&h, (h.key(0), 0), (a, implicit_user), 0, deposit, &[]);
        rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
    }
    for (from, to) in [(user, implicit_user), (implicit_user, user)] {
        let i = transfer(&h, e.root, h.key(0), (a, from), (a, to), 0, &[]);
        rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
    }
    assert!(h.svm.get_account(&child(a, implicit_user)).is_none());
    // A registered subgroup controls its own immediate children.
    let subgroup = child(a, implicit_user);
    let i = group(&h, e.root, h.key(0), a, implicit_user, true, &[]);
    succeeds(&mut h, &[0], i);
    let i = e.movement(&h, (h.key(0), 0), (subgroup, user), 20, true, &[a]);
    succeeds(&mut h, &[0], i);
    let i = transfer(&h, e.root, h.key(0), (a, user), (subgroup, user), 10, &[]);
    succeeds(&mut h, &[0], i);
    let i = e.movement(&h, (h.key(0), 0), (subgroup, user), 30, false, &[a]);
    succeeds(&mut h, &[0], i);
    assert!(!h.record(child(subgroup, user)).registered);
    assert_eq!(h.record(child(subgroup, user)).debit, 0);
    assert_eq!(h.record(a).debit, 90);
    assert_eq!((h.record(e.root).debit, h.record(e.root).credit), (90, 90));
}

#[test]
fn no_op_removal_still_requires_a_registered_group_in_the_selected_ledger() {
    let mut h = Harness::new();
    let (root, _) = h.internal();
    let absent_parent = Address::new_from_array([90; 32]);
    let i = remove(&h, root, h.key(0), absent_parent, h.key(1), false);
    rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let user = h.key(1);
    let i = leaf(&h, e.root, h.key(0), a, user, "Leaf", false);
    succeeds(&mut h, &[0], i);
    let mut i = remove(&h, e.root, h.key(0), child(a, user), h.key(2), false);
    i.accounts.push(AccountMeta::new_readonly(a, false));
    rejects(&mut h, &[0], i, LedgerError::InvalidAccount.into());
    let restricted = branch(&mut h, e.root, 1, false);
    let i = remove(&h, e.root, h.key(1), restricted, h.key(2), false);
    succeeds(&mut h, &[0, 1], i); // Admission restricts monetary use, not a valid no-op removal.
}

#[test]
fn malformed_records_omitted_ancestors_duplicates_and_wrong_addresses_reject() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let user = h.key(1);
    let valid = e.movement(&h, (h.key(0), 0), (a, user), 1, true, &[]);
    let mut duplicate = valid.clone();
    duplicate.accounts.push(AccountMeta::new(a, false));
    rejects(&mut h, &[0], duplicate, LedgerError::InvalidAccount.into());
    let mut omitted = valid.clone();
    omitted.accounts.retain(|m| m.pubkey != a);
    rejects(&mut h, &[0], omitted, LedgerError::InvalidAccount.into());
    let saved = h.svm.get_account(&a).unwrap();
    // Account substitution with a genuine record at a different key is not a PDA proof.
    let fake = Address::new_from_array([90; 32]);
    h.svm.set_account(fake, saved.clone()).unwrap();
    let mut substituted = valid.clone();
    for m in &mut substituted.accounts {
        if m.pubkey == a {
            m.pubkey = fake;
        }
    }
    rejects(
        &mut h,
        &[0],
        substituted,
        LedgerError::InvalidAccount.into(),
    );
    for malformed in 0..4 {
        let mut account = saved.clone();
        match malformed {
            0 => account.owner = TOKEN,
            1 => account.data[0] ^= 1,
            2 => account.data.truncate(20),
            _ => {
                let mut record = h.record(a);
                record.bump ^= 1;
                anchor_lang::AnchorSerialize::serialize(&record, &mut &mut account.data[8..])
                    .unwrap();
            }
        }
        h.svm.set_account(a, account).unwrap();
        rejects(
            &mut h,
            &[0],
            valid.clone(),
            LedgerError::InvalidAccount.into(),
        );
        h.svm.set_account(a, saved.clone()).unwrap();
    }
    succeeds(&mut h, &[0], valid); // Same valid accounts and request do succeed.
}

#[test]
fn token_mint_vault_authority_wallet_alias_and_program_substitutions_reject() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let other = External::new(&mut h, 70, 0);
    let a = branch(&mut h, e.root, 0, true);
    let user = h.key(1);
    let valid = e.movement(&h, (h.key(0), 0), (a, user), 1, true, &[]);
    for (from, to, code) in [
        (
            e.mint,
            other.mint,
            u32::from(ErrorCode::ConstraintTokenMint),
        ),
        (
            e.wallet,
            other.wallet,
            u32::from(ErrorCode::ConstraintTokenMint),
        ),
        (e.vault, other.vault, u32::from(ErrorCode::ConstraintSeeds)),
        (
            e.wallet,
            e.vault,
            u32::from(ErrorCode::ConstraintDuplicateMutableAccount),
        ),
        (TOKEN, SYSTEM, u32::from(ErrorCode::InvalidProgramId)),
    ] {
        let mut substituted = valid.clone();
        for m in &mut substituted.accounts {
            if m.pubkey == from {
                m.pubkey = to;
            }
        }
        rejects(&mut h, &[0], substituted, code);
    }
    let mut vault = TokenAccount::unpack(&h.svm.get_account(&e.vault).unwrap().data).unwrap();
    vault.owner = h.key(0);
    h.pack(e.vault, vault);
    rejects(&mut h, &[0], valid, ErrorCode::ConstraintTokenOwner.into());
}

#[test]
fn failed_token_settlement_and_late_commit_undo_new_leaf_allocation() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let user = h.key(1);
    let valid = e.movement(&h, (h.key(0), 0), (a, user), 10, true, &[]);
    let mut frozen = TokenAccount::unpack(&h.svm.get_account(&e.wallet).unwrap().data).unwrap();
    frozen.state = AccountState::Frozen;
    h.pack(e.wallet, frozen);
    rejects(
        &mut h,
        &[0],
        valid.clone(),
        spl_token_interface::error::TokenError::AccountFrozen as u32,
    );
    frozen.state = AccountState::Initialized;
    h.pack(e.wallet, frozen);
    let mut readonly = valid.clone();
    for m in &mut readonly.accounts {
        if m.pubkey == a {
            m.is_writable = false;
        }
    }
    rejects(&mut h, &[0], readonly, LedgerError::InvalidAccount.into());
    assert!(h.svm.get_account(&child(a, user)).is_none());
    assert_eq!((h.token(e.wallet), h.token(e.vault)), (1000, 0));
    succeeds(&mut h, &[0], valid);
    assert_eq!(h.record(child(a, user)).debit, 10);
}

#[test]
fn multiple_application_claims_share_one_source_and_donations_cannot_fund_claims() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let b = branch(&mut h, e.root, 1, true);
    let user = h.key(2);
    let i = e.movement(&h, (h.key(0), 0), (a, user), 100, true, &[]);
    succeeds(&mut h, &[0], i);
    let i = e.movement(&h, (h.key(1), 0), (b, user), 200, true, &[]);
    succeeds(&mut h, &[0, 1], i);
    let donation = spl_token_interface::instruction::transfer_checked(
        &TOKEN,
        &e.wallet,
        &e.mint,
        &e.vault,
        &h.key(0),
        &[],
        700,
        6,
    )
    .unwrap();
    succeeds(&mut h, &[0], donation);
    assert_eq!(h.token(e.vault), 1000);
    assert_eq!(h.record(a).debit + h.record(b).debit, 300);
    assert_eq!(
        (h.record(e.root).debit, h.record(e.root).credit),
        (300, 300)
    );
    assert_eq!(h.record(e.source).credit, 300);
    let i = e.movement(&h, (h.key(0), 0), (a, user), 1, true, &[]);
    rejects(
        &mut h,
        &[0],
        i,
        spl_token_interface::error::TokenError::InsufficientFunds as u32,
    );
    let i = e.movement(&h, (h.key(1), 0), (b, user), 200, false, &[]);
    succeeds(&mut h, &[0, 1], i);
    assert_eq!(h.token(e.vault), 800);
    assert_eq!(h.record(e.source).credit, 100);
    assert_eq!(h.record(a).debit, 100);
    assert_eq!(h.record(b).debit, 0);
}

#[test]
fn deposits_and_withdrawals_require_eligible_leaves_in_the_authorized_branch() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let a = branch(&mut h, e.root, 0, true);
    let b = branch(&mut h, e.root, 1, true);
    for deposit in [true, false] {
        for relative in [sa(SOURCE), h.key(0)] {
            let i = e.movement(&h, (h.key(0), 0), (e.root, relative), 0, deposit, &[]);
            rejects(&mut h, &[0], i, LedgerError::InvalidKind.into());
        }
        let i = e.movement(&h, (h.key(0), 0), (b, h.key(2)), 0, deposit, &[]);
        rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    }
    let i = e.movement(&h, (h.key(0), 0), (a, h.key(2)), 1, false, &[]);
    rejects(&mut h, &[0], i, LedgerError::Accounting.into());
}

#[test]
fn direct_implicit_holder_and_prefunded_storage_do_not_require_registration() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let holder = h.key(1);
    let account = child(e.root, holder);
    // Anyone can transfer lamports to an unallocated PDA. That must not capture it
    // or prevent the actual authority from using it; allocation tops up rent.
    h.svm
        .airdrop(&account, h.svm.minimum_balance_for_rent_exemption(0))
        .unwrap();
    let i = e.movement(&h, (holder, 0), (e.root, holder), 10, true, &[]);
    succeeds(&mut h, &[0, 1], i);
    assert_eq!(h.record(account).debit, 10);
    assert!(!h.record(account).registered);
    let i = e.movement(&h, (h.key(0), 0), (e.root, holder), 10, false, &[]);
    rejects(&mut h, &[0], i, LedgerError::Unauthorized.into());
    let i = e.movement(&h, (holder, 0), (e.root, holder), 10, false, &[]);
    succeeds(&mut h, &[0, 1], i);
    assert_eq!(h.record(account).debit, 0);
    assert_eq!((h.record(e.root).debit, h.record(e.root).credit), (0, 0));
}

#[test]
fn internal_u128_overflow_rolls_back_and_does_not_affect_external_claims() {
    let mut h = Harness::new();
    let e = External::new(&mut h, 60, 0);
    let external_before = h.svm.get_account(&e.root_storage).unwrap();
    let (root, source) = h.internal();
    let user = h.key(1);
    let i = transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (root, user),
        u128::MAX,
        &[],
    );
    succeeds(&mut h, &[0], i);
    assert_eq!(
        (h.record(root).debit, h.record(root).credit),
        (u128::MAX, u128::MAX)
    );
    let i = transfer(
        &h,
        root,
        h.key(0),
        (root, sa(SOURCE)),
        (root, h.key(2)),
        1,
        &[],
    );
    rejects(&mut h, &[0], i, LedgerError::Accounting.into());
    assert_eq!(h.record(source).credit, u128::MAX);
    assert!(h.svm.get_account(&child(root, h.key(2))).is_none());
    let i = transfer(
        &h,
        root,
        h.key(0),
        (root, user),
        (root, sa(SOURCE)),
        u128::MAX,
        &[],
    );
    succeeds(&mut h, &[0], i);
    assert_eq!((h.record(root).debit, h.record(root).credit), (0, 0));
    assert_eq!(h.svm.get_account(&e.root_storage).unwrap(), external_before);
    assert_eq!(h.token(e.vault), 0);
}
