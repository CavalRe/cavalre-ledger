// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {TestBase, Ledgers} from "./TestBase.sol";

contract AccountingTest is TestBase {
    function testDirectTransferNeedsOnlySourceButChecksSelfTransferFunds() public {
        _post(source, alice, 10);
        _unauthorized(ALICE);
        ledger.transferJournal(alice, bob, 0);
        vm.prank(ALICE);
        ledger.transferJournal(alice, bob, 7);
        _balance(alice, 3, 0);
        _balance(bob, 7, 0);
        _balance(journal, 10, 10);
        vm.expectRevert(Ledgers.InsufficientBalance.selector);
        vm.prank(ALICE);
        ledger.transferJournal(alice, alice, 4);
        vm.prank(ALICE);
        ledger.postJournal(alice, alice, type(uint128).max);
        _balance(alice, 3, 0);
    }

    function testGroupsCreditLeavesAndNestedJournalAccountsCannotUseDirectTransfer() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), address(this), Ledgers.Kind.DebitGroup);
        bytes32 nested_ = _create(journal, group_, bytes32("nested"), ALICE, Ledgers.Kind.DebitLeaf);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, group_));
        ledger.postJournal(group_, alice, 0);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, source));
        ledger.transferJournal(source, alice, 0);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, nested_));
        vm.prank(ALICE);
        ledger.transferJournal(nested_, alice, 0);
        _post(source, nested_, 10);
        _post(nested_, alice, 4);
        _balance(group_, 6, 0);
        _balance(alice, 4, 0);
    }

    function testMaximumDepthAndImmutableCompleteAncestry() public {
        bytes32 parent_ = journal;
        for (uint256 i_; i_ < 15; ++i_) {
            parent_ = _create(journal, parent_, bytes32(i_), address(this), Ledgers.Kind.CreditGroup);
        }
        bytes32 leaf_ = _create(journal, parent_, bytes32("leaf"), address(this), Ledgers.Kind.DebitLeaf);
        require(ledger.account(leaf_).depth == 16, "max depth");
        _post(source, leaf_, 10);
        _balance(parent_, 10, 0);
        _balance(journal, 10, 10);
        bytes32 deepest_ = _create(journal, parent_, bytes32("group"), address(this), Ledgers.Kind.DebitGroup);
        vm.expectRevert(Ledgers.DepthLimit.selector);
        ledger.createNode(journal, deepest_, bytes32("too deep"), address(this), Ledgers.Kind.DebitLeaf);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, leaf_));
        ledger.createNode(journal, leaf_, bytes32("child"), address(this), Ledgers.Kind.DebitLeaf);
    }

    function testCloseRequiresZeroGrossSidesAndNoChildren() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), address(this), Ledgers.Kind.DebitGroup);
        bytes32 debit_ = _create(journal, group_, bytes32("debit"), ALICE, Ledgers.Kind.DebitLeaf);
        bytes32 credit_ = _create(journal, group_, bytes32("credit"), BOB, Ledgers.Kind.CreditLeaf);
        _post(credit_, debit_, 10);
        _balance(group_, 10, 10);
        vm.expectRevert(Ledgers.NonemptyAccount.selector);
        ledger.closeNode(group_);
        vm.expectRevert(Ledgers.NonemptyAccount.selector);
        vm.prank(ALICE);
        ledger.closeNode(debit_);
        _post(debit_, credit_, 10);
        vm.expectRevert(Ledgers.NonemptyAccount.selector);
        ledger.closeNode(group_);
        vm.prank(ALICE);
        ledger.closeNode(debit_);
        require(ledger.account(group_).children == 1, "child decrement");
        vm.prank(BOB);
        ledger.closeNode(credit_);
        ledger.closeNode(group_);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, group_));
        ledger.account(group_);
    }

    function testSameSideSharedAncestorsCancelAtU128Limit() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), address(this), Ledgers.Kind.CreditGroup);
        bytes32 a_ = _create(journal, group_, bytes32("a"), address(this), Ledgers.Kind.DebitLeaf);
        bytes32 b_ = _create(journal, group_, bytes32("b"), address(this), Ledgers.Kind.DebitLeaf);
        _post(source, a_, type(uint128).max);
        _post(a_, b_, type(uint128).max);
        _balance(group_, type(uint128).max, 0);
        _balance(journal, type(uint128).max, type(uint128).max);
        _balance(a_, 0, 0);
        _balance(b_, type(uint128).max, 0);
        vm.expectRevert(Ledgers.Overflow.selector);
        ledger.postJournal(source, a_, 1);
        _balance(a_, 0, 0);
    }

    function testOppositeSidesPreserveGrossBalancesUnderCreditGroup() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), address(this), Ledgers.Kind.CreditGroup);
        bytes32 credit_ = _create(journal, group_, bytes32("credit"), address(this), Ledgers.Kind.CreditLeaf);
        bytes32 debit_ = _create(journal, group_, bytes32("debit"), address(this), Ledgers.Kind.DebitLeaf);
        _post(credit_, debit_, 30);
        _balance(group_, 30, 30);
        _post(debit_, credit_, 7);
        _balance(group_, 23, 23);
        _balance(journal, 23, 23);
    }

    function testCrossRootAndUnregisteredAccountsRejected() public {
        bytes32 other_ = ledger.createJournal(ns, bytes32("other"));
        bytes32 leaf_ = _create(other_, other_, bytes32("leaf"), address(this), Ledgers.Kind.DebitLeaf);
        vm.expectRevert(Ledgers.DifferentRoots.selector);
        ledger.postJournal(source, leaf_, 0);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, alice));
        ledger.createNode(other_, alice, bytes32("child"), address(this), Ledgers.Kind.DebitLeaf);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, bytes32("missing")));
        ledger.postJournal(bytes32("missing"), alice, 0);
    }

    function testFuzzIssueTransferRedeemConservesGross(uint128 issued_, uint128 proposed_) public {
        uint128 moved_ = proposed_ > issued_ ? issued_ : proposed_;
        _post(source, alice, issued_);
        vm.prank(ALICE);
        ledger.transferJournal(alice, bob, moved_);
        _balance(journal, issued_, issued_);
        _balance(alice, issued_ - moved_, 0);
        _balance(bob, moved_, 0);
        _post(bob, source, moved_);
        _balance(journal, issued_ - moved_, issued_ - moved_);
        _balance(source, 0, issued_ - moved_);
    }
}
