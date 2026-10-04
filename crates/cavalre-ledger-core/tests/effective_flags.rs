use cavalre_ledger_core::ledger_lib::{self, *};
type Flags = ledger_lib::Flags<u64>;
use std::collections::BTreeMap;

#[derive(Default)]
struct MemoryStore {
    flags: BTreeMap<u64, Flags>,
    custody: BTreeMap<u64, u64>,
    relatives: BTreeMap<u64, u64>,
}

impl Store<u64> for MemoryStore {
    fn flags(&self, absolute: &u64) -> Result<Option<Flags>, Error> {
        Ok(self.flags.get(absolute).copied())
    }

    fn custody_account(&self, absolute: &u64) -> Result<Option<u64>, Error> {
        Ok(self.custody.get(absolute).copied())
    }

    fn relative(&self, absolute: &u64) -> Result<u64, Error> {
        self.relatives
            .get(absolute)
            .copied()
            .ok_or(Error::InvalidAddress)
    }
}

fn key(n: u8) -> u64 {
    u64::from(n)
}

fn root() -> MemoryStore {
    let mut store = MemoryStore::default();
    store.flags.insert(
        key(1),
        Flags {
            parent: key(0),
            account_kind: AccountKind::DebitGroup,
            token_kind: TokenKind::External,
            depth: 2,
        },
    );
    store
}

fn group(store: &mut MemoryStore, credit: bool) -> u64 {
    let absolute = TestAddresses.to_address(&key(1), &key(2));
    store.flags.insert(
        absolute,
        Flags {
            parent: key(1),
            account_kind: if credit {
                AccountKind::CreditGroup
            } else {
                AccountKind::DebitGroup
            },
            token_kind: TokenKind::Unregistered,
            depth: 3,
        },
    );
    store.custody.insert(absolute, absolute);
    store.relatives.insert(absolute, key(2));
    absolute
}

#[test]
fn direct_implicit_leaf_needs_no_registration() {
    let store = root();
    let (effective, original, absolute) =
        effective_flags(&store, &TestAddresses, &key(1), &key(1), &key(3)).unwrap();
    assert_eq!(original, None);
    assert_eq!(effective.account_kind, AccountKind::DebitLedger);
    assert_eq!(effective.token_kind, TokenKind::Unregistered);
    assert_eq!(effective.depth, 3);
    assert_eq!(store.flags.len(), 1);
    assert_eq!(ledger(&store, &absolute), Ok(None));
    assert_eq!(
        custody(&store, &key(1), effective, &key(3)),
        Ok((key(3), false))
    );
}

#[test]
fn nested_implicit_leaves_inherit_both_polarities_and_custody() {
    for credit in [false, true] {
        let mut store = root();
        let parent = group(&mut store, credit);
        let (effective, original, _) =
            effective_flags(&store, &TestAddresses, &key(1), &parent, &key(3)).unwrap();
        assert_eq!(original, None);
        assert_eq!(effective.account_kind.is_credit(), credit);
        assert!(!effective.account_kind.is_group());
        assert_eq!(effective.depth, 4);
        assert_eq!(
            custody(&store, &key(1), effective, &key(3)),
            Ok((key(2), credit))
        );
    }
}

#[test]
fn registered_leaf_keeps_its_own_polarity() {
    let mut store = root();
    let parent = group(&mut store, false);
    let absolute = TestAddresses.to_address(&parent, &key(3));
    let original = Flags {
        parent,
        account_kind: AccountKind::CreditLedger,
        token_kind: TokenKind::Unregistered,
        depth: 4,
    };
    store.flags.insert(absolute, original);
    assert_eq!(
        effective_flags(&store, &TestAddresses, &key(1), &parent, &key(3)),
        Ok((original, Some(original), absolute))
    );
    // Custodian polarity is independent of the leaf's polarity.
    assert_eq!(
        custody(&store, &key(1), original, &key(3)),
        Ok((key(2), false))
    );
}

#[test]
fn rejects_wrong_root_and_non_group_parent() {
    let mut store = root();
    assert_eq!(
        effective_flags(&store, &TestAddresses, &key(8), &key(1), &key(3)),
        Err(Error::DifferentRoots)
    );
    assert_eq!(
        effective_flags(&store, &TestAddresses, &key(1), &key(7), &key(3)),
        Err(Error::InvalidAccountGroup)
    );
    store.flags.get_mut(&key(1)).unwrap().account_kind = AccountKind::DebitLedger;
    assert_eq!(
        effective_flags(&store, &TestAddresses, &key(1), &key(1), &key(3)),
        Err(Error::InvalidAccountGroup)
    );
}

#[test]
fn rejects_depth_overflow() {
    let mut store = root();
    let parent = group(&mut store, false);
    store.flags.get_mut(&parent).unwrap().depth = u8::MAX;
    assert_eq!(
        effective_flags(&store, &TestAddresses, &key(1), &parent, &key(3)),
        Err(Error::DepthOverflow)
    );
}

// Test-only address scheme, deliberately independent of Solana. Production
// hosts choose and authenticate their own collision-resistant derivation.
struct TestAddresses;
impl AddressDerivation<u64> for TestAddresses {
    fn to_address(&self, parent: &u64, relative: &u64) -> u64 {
        parent * 256 + relative
    }
}
