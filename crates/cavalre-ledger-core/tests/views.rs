//! This suite also runs with the mutating ledger module excluded from the build.
use cavalre_ledger_core::{
    ledger_lib::{
        Account, AccountKind, AddressDerivation, Balances, Error, Flags, ReadStore, TokenKind,
    },
    ledger_view::{self as view, ChildIndex, LedgerIndex},
};
use std::collections::BTreeMap;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

// Count only the measured test thread, so parallel tests cannot affect results.
thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
struct CountAllocations;
fn allocation() {
    let _ = ALLOCATIONS.try_with(|count| {
        if let Some(n) = count.get() {
            count.set(Some(n + 1));
        }
    });
}
unsafe impl GlobalAlloc for CountAllocations {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation();
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: CountAllocations = CountAllocations;

fn measured<T>(f: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATIONS.with(|count| count.set(Some(0)));
    let result = f();
    let count = ALLOCATIONS.with(|count| count.replace(None).unwrap());
    (result, count)
}

const ROOT: u64 = 1;
const APP: u64 = 10;
const SOURCE: u64 = 83;
fn addr(parent: u64, relative: u64) -> u64 {
    parent * 256 + relative
}
#[derive(Clone)]
struct Snapshot {
    records: BTreeMap<u64, Option<Account<u64>>>,
    roots: Vec<u64>,
}
impl AddressDerivation<u64> for Snapshot {
    fn to_address(&self, parent: &u64, relative: &u64) -> u64 {
        addr(*parent, *relative)
    }
}
impl ReadStore<u64> for Snapshot {
    fn account(&self, key: &u64) -> Result<Option<Account<u64, &str>>, Error> {
        self.records
            .get(key)
            .map(|a| a.as_ref().map(Account::as_ref))
            .ok_or(Error::MissingAccount)
    }
}
impl ChildIndex<u64> for Snapshot {
    fn child_addresses(&self, parent: &u64) -> Result<Vec<u64>, Error> {
        Ok(self
            .records
            .iter()
            .filter_map(|(k, a)| {
                a.as_ref()
                    .filter(|a| a.registered && a.flags.depth > 2 && a.flags.parent == *parent)
                    .map(|_| *k)
            })
            .collect())
    }
}
impl LedgerIndex<u64> for Snapshot {
    fn ledger_addresses(&self) -> Result<Vec<u64>, Error> {
        Ok(self.roots.clone())
    }
}
fn record(
    parent: u64,
    relative: u64,
    kind: AccountKind,
    depth: u8,
    custodian: u64,
    balance: u128,
    registered: bool,
) -> Account<u64> {
    Account {
        flags: Flags {
            parent,
            account_kind: kind,
            depth,
            token_kind: if depth == 2 {
                TokenKind::External
            } else {
                TokenKind::Unregistered
            },
        },
        relative,
        custodian,
        registered,
        implicit_allowed: true,
        children: 0,
        balances: if kind.is_credit() {
            Balances {
                debit: 0,
                credit: balance,
            }
        } else {
            Balances {
                debit: balance,
                credit: 0,
            }
        },
        name: if registered {
            "Account".into()
        } else {
            String::new()
        },
    }
}
fn snapshot() -> Snapshot {
    let app = addr(ROOT, APP);
    let source = addr(ROOT, SOURCE);
    let mut root = record(0, ROOT, AccountKind::DebitGroup, 2, 0, 150, true);
    root.balances.credit = 150;
    root.children = 2;
    let mut group = record(ROOT, APP, AccountKind::DebitGroup, 3, app, 150, true);
    group.children = 1;
    Snapshot {
        roots: vec![ROOT],
        records: BTreeMap::from([
            (ROOT, Some(root)),
            (
                source,
                Some(record(
                    ROOT,
                    SOURCE,
                    AccountKind::CreditLedger,
                    3,
                    source,
                    150,
                    true,
                )),
            ),
            (app, Some(group)),
            (
                addr(app, 20),
                Some(record(app, 20, AccountKind::DebitLedger, 4, app, 80, true)),
            ),
            (
                addr(app, 21),
                Some(record(app, 21, AccountKind::DebitLedger, 4, app, 70, false)),
            ),
            (addr(app, 22), None),
        ]),
    }
}

#[test]
fn field_reads_allocate_nothing_and_posting_allocates_one_change_buffer() {
    use cavalre_ledger_core::ledger_lib::{self as lib, AccountingStore, Store, StoreView};
    let mut state = snapshot();
    for account in state.records.values_mut().flatten() {
        account.name = "N".repeat(64);
    }
    let store = StoreView(&state);
    let app = addr(ROOT, APP);
    let leaf = addr(app, 20);
    let (_, count) = measured(|| {
        for _ in 0..1000 {
            assert_eq!(store.flags(&leaf).unwrap().unwrap().depth, 4);
            assert_eq!(store.relative(&leaf).unwrap(), 20);
            assert_eq!(store.custody_account(&leaf).unwrap(), Some(app));
            assert_eq!(store.balances(&leaf).unwrap().debit, 80);
            assert_eq!(view::balance_of(&state, &ROOT, &app, &20).unwrap(), 80);
            assert_eq!(view::balance_of(&state, &ROOT, &app, &21).unwrap(), 70);
            assert_eq!(view::total_supply(&state, &ROOT).unwrap(), 150);
        }
    });
    assert_eq!(count, 0, "simple reads allocated account metadata");
    let from = lib::Endpoint {
        relative: 20,
        flags: lib::effective_flags(&store, &store, &ROOT, &app, &20)
            .unwrap()
            .0,
    };
    let to = lib::Endpoint {
        relative: 21,
        flags: lib::effective_flags(&store, &store, &ROOT, &app, &21)
            .unwrap()
            .0,
    };
    let (changes, count) =
        measured(|| lib::transfer_debits(&store, &store, &ROOT, from, to, &APP, 10).unwrap());
    assert_eq!(
        count, 1,
        "posting allocated beyond its bounded change buffer"
    );
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].after.debit, 70);
    assert_eq!(changes[1].after.debit, 80);
}

#[test]
fn reads_registered_and_implicit_properties_without_any_mutation_host() {
    let state = snapshot();
    let before = state.records.clone();
    let app = addr(ROOT, APP);
    let registered = view::account(&state, &ROOT, &app, &20).unwrap();
    assert!(registered.registered);
    assert_eq!((registered.custodian, registered.balances.debit), (APP, 80));
    let implicit = view::account(&state, &ROOT, &app, &21).unwrap();
    assert!(!implicit.registered);
    assert_eq!(implicit.flags.account_kind, AccountKind::DebitLedger);
    assert_eq!(
        (
            implicit.custodian,
            implicit.flags.depth,
            implicit.balances.debit
        ),
        (APP, 4, 70)
    );
    assert_eq!(view::ledger(&state, &addr(app, 20)).unwrap(), Some(ROOT));
    assert_eq!(view::ledger(&state, &addr(app, 21)).unwrap(), None);
    assert_eq!(state.records, before);
}
#[test]
fn restricted_implicit_leaves_remain_inspectable() {
    let mut state = snapshot();
    let app = addr(ROOT, APP);
    state
        .records
        .get_mut(&app)
        .unwrap()
        .as_mut()
        .unwrap()
        .implicit_allowed = false;
    let leaf = view::account(&state, &ROOT, &app, &21).unwrap();
    assert!(!leaf.admitted);
    assert_eq!(view::balance_of(&state, &ROOT, &app, &21).unwrap(), 70);
    assert!(view::account(&state, &ROOT, &app, &20).unwrap().admitted);
}
#[test]
fn confirmed_absence_is_zero_but_missing_input_is_an_error() {
    let state = snapshot();
    let app = addr(ROOT, APP);
    assert_eq!(view::balance_of(&state, &ROOT, &app, &22).unwrap(), 0);
    assert_eq!(view::name(&state, &addr(app, 22)).unwrap(), "");
    assert_eq!(
        view::account(&state, &ROOT, &app, &23),
        Err(Error::MissingAccount)
    );
}
#[test]
fn gross_net_and_supply_follow_original_solidity_semantics() {
    let mut state = snapshot();
    let app = addr(ROOT, APP);
    assert_eq!(view::total_supply(&state, &ROOT).unwrap(), 150); // Root net is zero.
    assert_eq!(
        view::balance_of(&state, &ROOT, &ROOT, &SOURCE).unwrap(),
        150
    );
    let account = state
        .records
        .get_mut(&addr(app, 20))
        .unwrap()
        .as_mut()
        .unwrap();
    account.balances.credit = 20;
    assert_eq!(
        view::debit_balance_of(&state, &ROOT, &app, &20).unwrap(),
        80
    );
    assert_eq!(
        view::credit_balance_of(&state, &ROOT, &app, &20).unwrap(),
        20
    );
    assert_eq!(view::balance_of(&state, &ROOT, &app, &20).unwrap(), 60);
    state
        .records
        .get_mut(&addr(app, 20))
        .unwrap()
        .as_mut()
        .unwrap()
        .balances
        .credit = 90;
    assert_eq!(
        view::balance_of(&state, &ROOT, &app, &20),
        Err(Error::InsufficientBalance)
    );
    assert_eq!(
        view::credit_balance_of(&state, &ROOT, &app, &20).unwrap(),
        90
    );
}
#[test]
fn children_exclude_implicit_leaves_and_reject_incomplete_snapshots() {
    let mut state = snapshot();
    let app = addr(ROOT, APP);
    assert_eq!(
        view::sub_accounts(&state, &ROOT, &app, 0, usize::MAX).unwrap(),
        vec![20]
    );
    assert_eq!(view::sub_account(&state, &ROOT, &app, 0).unwrap(), 20);
    assert!(
        view::sub_accounts(&state, &ROOT, &app, usize::MAX, usize::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        view::sub_account(&state, &ROOT, &app, 1),
        Err(Error::InvalidIndex)
    );
    state.records.remove(&addr(app, 20));
    assert_eq!(
        view::sub_accounts(&state, &ROOT, &app, 0, 10),
        Err(Error::IncompleteIndex)
    );
}
#[test]
fn ledger_enumeration_validates_roots_duplicates_and_page_bounds() {
    let mut state = snapshot();
    assert_eq!(view::ledger_count(&state).unwrap(), 1);
    assert_eq!(view::ledger_at(&state, 0).unwrap(), ROOT);
    assert_eq!(view::ledger_at(&state, 1), Err(Error::InvalidIndex));
    assert_eq!(view::ledgers(&state, 0, usize::MAX).unwrap(), vec![ROOT]);
    assert!(view::ledgers(&state, usize::MAX, 1).unwrap().is_empty());
    state.roots.push(ROOT);
    assert_eq!(view::ledger_count(&state), Err(Error::InvalidIndex));
}
#[test]
fn queries_validate_root_parent_and_registered_leaf_eligibility_as_parent() {
    let state = snapshot();
    let app = addr(ROOT, APP);
    assert_eq!(
        view::account(&state, &app, &app, &20),
        Err(Error::InvalidAccount)
    );
    assert_eq!(
        view::account(&state, &ROOT, &addr(app, 20), &1),
        Err(Error::InvalidAccountGroup)
    );
    assert_eq!(view::total_supply(&state, &app), Err(Error::InvalidAccount));
}
