//! This suite also runs with the mutating ledger module excluded from the build.
use cavalre_ledger_core::{
    ledger_lib::{
        Account, AccountKind, AddressDerivation, Balances, Error, Flags, ReadStore, TokenKind,
    },
    ledger_view::{self as view, ChildIndex},
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
    children: BTreeMap<(u64, u32), u64>,
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
    fn child_at(&self, parent: &u64, index: u32) -> Result<u64, Error> {
        self.children
            .get(&(*parent, index))
            .copied()
            .ok_or(Error::IncompleteIndex)
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
        sub_index: u32::from(registered),
        symbol: if depth == 2 {
            "UNIT".into()
        } else {
            String::new()
        },
        decimals: 0,
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
    let mut global = record(0, 0, AccountKind::DebitGroup, 1, 0, 0, true);
    global.children = 1;
    Snapshot {
        children: BTreeMap::from([
            ((0, 0), ROOT),
            ((ROOT, 0), SOURCE),
            ((ROOT, 1), APP),
            ((app, 0), 20),
        ]),
        records: BTreeMap::from([
            (0, Some(global)),
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
fn transfer_write_planning_preserves_cancellation_and_distinguishes_absent_storage() {
    use cavalre_ledger_core::ledger_lib::Child;
    let mut state = snapshot();
    let app = addr(ROOT, APP);
    let from = Child {
        parent: app,
        relative: 20,
    };
    let to = Child {
        parent: app,
        relative: 21,
    };
    assert_eq!(
        view::transfer_writable_accounts(&state, &ROOT, from, to, 10).unwrap(),
        vec![addr(app, 20), addr(app, 21)]
    );
    assert!(view::transfer_writable_accounts(&state, &ROOT, from, to, 0)
        .unwrap()
        .is_empty());
    assert!(
        view::transfer_writable_accounts(&state, &ROOT, from, from, 10)
            .unwrap()
            .is_empty()
    );
    let absent = Child {
        parent: app,
        relative: 22,
    };
    assert!(
        view::transfer_writable_accounts(&state, &ROOT, from, absent, 0)
            .unwrap()
            .is_empty()
    );
    assert!(
        view::transfer_writable_accounts(&state, &ROOT, absent, absent, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        view::transfer_writable_accounts(&state, &ROOT, from, absent, 10).unwrap(),
        vec![addr(app, 20), addr(app, 22)]
    );
    let source = Child {
        parent: ROOT,
        relative: SOURCE,
    };
    let writes = view::transfer_writable_accounts(&state, &ROOT, source, from, 10).unwrap();
    assert!(writes.contains(&ROOT));
    assert!(writes.contains(&app));
    state.records.remove(&addr(app, 22));
    assert_eq!(
        view::transfer_writable_accounts(&state, &ROOT, from, absent, 10),
        Err(Error::MissingAccount)
    );
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
    assert_eq!(view::sub_account(&state, &ROOT, &app, 0), Ok(20));
    state.children.remove(&(app, 0));
    assert_eq!(
        view::sub_accounts(&state, &ROOT, &app, 0, 10),
        Err(Error::IncompleteIndex)
    );
}
#[test]
fn ledger_discovery_is_root_child_enumeration_with_the_same_page_bounds() {
    let mut state = snapshot();
    assert_eq!(
        view::ledger_count(&state, &0),
        view::sub_account_count(&state, &0, &0)
    );
    assert_eq!(
        view::ledger_at(&state, &0, 0),
        view::sub_account(&state, &0, &0, 0)
    );
    assert_eq!(view::ledger_at(&state, &0, 0), Ok(ROOT));
    assert_eq!(view::ledger_at(&state, &0, 1), Err(Error::InvalidIndex));
    assert_eq!(
        view::ledgers(&state, &0, 0, usize::MAX),
        view::sub_accounts(&state, &0, &0, 0, usize::MAX)
    );
    assert!(view::ledgers(&state, &0, usize::MAX, 1).unwrap().is_empty());
    assert!(view::ledgers(&state, &0, 0, 0).unwrap().is_empty());
    state.records.remove(&ROOT);
    assert_eq!(view::ledger_at(&state, &0, 0), Ok(ROOT));
    state.children.remove(&(0, 0));
    assert_eq!(view::ledger_count(&state, &0), Ok(1));
    assert_eq!(view::ledgers(&state, &0, 0, 1), Err(Error::IncompleteIndex));
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

#[test]
fn metadata_queries_read_stored_ledger_values_including_zero_decimals() {
    let mut state = snapshot();
    assert_eq!(view::symbol(&state, &ROOT), Ok(Some("UNIT".into())));
    assert_eq!(view::decimals(&state, &ROOT), Ok(Some(0)));
    assert_eq!(
        view::symbol(&state, &addr(ROOT, APP)),
        Err(Error::InvalidAccount)
    );
    assert_eq!(
        view::decimals(&state, &addr(ROOT, APP)),
        Err(Error::InvalidAccount)
    );
    state
        .records
        .get_mut(&ROOT)
        .unwrap()
        .as_mut()
        .unwrap()
        .decimals = 255;
    assert_eq!(view::decimals(&state, &ROOT), Ok(Some(255)));
}

#[test]
fn page_reads_touch_only_requested_slots_even_for_a_large_parent() {
    let mut state = snapshot();
    state
        .records
        .get_mut(&0)
        .unwrap()
        .as_mut()
        .unwrap()
        .children = u32::MAX;
    state.records.remove(&ROOT); // No child records supplied.
    state.children.clear();
    state.children.insert((0, 2_000_000), 99);
    state.children.insert((0, 2_000_001), 77);
    assert_eq!(view::ledgers(&state, &0, 2_000_000, 2), Ok(vec![99, 77]));
    assert_eq!(view::ledger_at(&state, &0, 2_000_001), Ok(77));
    assert_eq!(
        view::ledgers(&state, &0, 2_000_000, 3),
        Err(Error::IncompleteIndex)
    );
    assert_eq!(view::ledgers(&state, &0, 0, 0), Ok(vec![]));
    assert_eq!(
        view::ledgers(&state, &0, usize::MAX, usize::MAX),
        Ok(vec![])
    );
}
