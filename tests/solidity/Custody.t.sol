// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {TestBase, Ledgers, TestToken, LedgerVault, LedgerToken} from "./TestBase.sol";

contract CustodyTest is TestBase {
    TestToken private token;
    bytes32 private asset;
    bytes32 private a;
    bytes32 private b;
    address private vault;

    function setUp() public override {
        super.setUp();
        (token, asset, a, b) = _asset();
        vault = ledger.root(asset).vault;
    }

    function testDepositTransferWithdrawAndDonations() public {
        vm.prank(BOB);
        token.transfer(vault, 50);
        _balance(asset, 0, 0);
        vm.prank(ALICE);
        ledger.deposit(a, 100);
        vm.prank(ALICE);
        ledger.transferClaims(a, b, 30);
        vm.prank(BOB);
        ledger.withdraw(b, 30);
        vm.prank(ALICE);
        ledger.withdraw(a, 70);
        _balance(asset, 0, 0);
        require(token.balanceOf(vault) == 50, "donation retained");
        require(token.balanceOf(ALICE) == 9970 && token.balanceOf(BOB) == 9980, "wallet settlement");
    }

    function testNamespaceAndParentCannotSpendOrRedeemDescendantClaims() public {
        bytes32 group_ = _create(asset, asset, bytes32("group"), PARENT, Ledgers.Kind.CreditGroup);
        bytes32 leaf_ = _create(asset, group_, bytes32("leaf"), ALICE, Ledgers.Kind.DebitLeaf);
        vm.prank(ALICE);
        ledger.deposit(a, 100);
        vm.prank(ALICE);
        ledger.transferClaims(a, leaf_, 100);
        _balance(group_, 100, 0);
        _unauthorized(ALICE);
        ledger.transferClaims(leaf_, b, 1);
        _unauthorized(ALICE);
        vm.prank(PARENT);
        ledger.transferClaims(leaf_, b, 0);
        vm.expectRevert(abi.encodeWithSelector(Ledgers.InvalidAccount.selector, leaf_));
        vm.prank(ALICE);
        ledger.withdraw(leaf_, 1);
        vm.prank(ALICE);
        ledger.transferClaims(leaf_, a, 100);
        vm.prank(ALICE);
        ledger.withdraw(a, 100);
        _balance(group_, 0, 0);
    }

    function testRootTypesPreventUnbackedClaimsAndJournalRedemption() public {
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        ledger.createNode(asset, asset, bytes32("credit"), address(this), Ledgers.Kind.CreditLeaf);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        vm.prank(ALICE);
        ledger.postJournal(a, b, 0);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        vm.prank(ALICE);
        ledger.transferJournal(a, b, 0);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        vm.prank(ALICE);
        ledger.transferClaims(alice, bob, 0);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        vm.prank(ALICE);
        ledger.deposit(alice, 0);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        vm.prank(ALICE);
        ledger.withdraw(alice, 0);
        vm.expectRevert(Ledgers.UnsupportedRoot.selector);
        ledger.openPosition(journal);
    }

    function testWithdrawRequiresControllerAndFullLiabilitySolvency() public {
        vm.prank(ALICE);
        ledger.deposit(a, 100);
        _unauthorized(ALICE);
        ledger.withdraw(a, 0);
        _unauthorized(ALICE);
        ledger.deposit(a, 0);
        token.burn(vault, 1);
        for (uint128 amount_; amount_ < 2; ++amount_) {
            vm.expectRevert(Ledgers.Undercollateralized.selector);
            vm.prank(ALICE);
            ledger.withdraw(a, amount_);
        }
        _balance(a, 100, 0);
        _balance(asset, 100, 100);
    }

    function testVaultsAreIsolatedBetweenNamespaces() public {
        bytes32 otherNs_ = ledger.createNamespace(bytes32("other"));
        bytes32 otherRoot_ = ledger.registerAsset(otherNs_, address(token));
        require(ledger.root(otherRoot_).vault != vault, "vault separation");
        vm.prank(ALICE);
        bytes32 otherPosition_ = ledger.openPosition(otherRoot_);
        vm.prank(ALICE);
        ledger.deposit(a, 100);
        vm.prank(ALICE);
        ledger.deposit(otherPosition_, 200);
        token.burn(vault, 1);
        vm.expectRevert(Ledgers.Undercollateralized.selector);
        vm.prank(ALICE);
        ledger.withdraw(a, 1);
        vm.prank(ALICE);
        ledger.withdraw(otherPosition_, 200);
        vm.expectRevert(LedgerVault.Unauthorized.selector);
        LedgerVault(vault).send(ALICE, 1);
    }

    function testRejectsRecipientFeesSenderFeesNoMovementAndFalseReturnsAtomically() public {
        for (uint256 i_ = 1; i_ <= 4; ++i_) {
            token.setMode(TestToken.Mode(i_));
            vm.expectRevert(i_ == 4 ? LedgerToken.TokenTransferFailed.selector : Ledgers.UnexpectedTokenDelta.selector);
            vm.prank(ALICE);
            ledger.deposit(a, 10);
            _balance(asset, 0, 0);
            require(token.balanceOf(ALICE) == 10000 && token.balanceOf(vault) == 0, "deposit rollback");
        }
        token.setMode(TestToken.Mode.Normal);
        vm.prank(ALICE);
        ledger.deposit(a, 100);
        for (uint256 i_ = 1; i_ <= 4; ++i_) {
            token.setMode(TestToken.Mode(i_));
            vm.expectRevert(i_ == 4 ? LedgerToken.TokenTransferFailed.selector : Ledgers.UnexpectedTokenDelta.selector);
            vm.prank(ALICE);
            ledger.withdraw(a, 10);
            _balance(asset, 100, 100);
            _balance(a, 100, 0);
            require(token.balanceOf(ALICE) == 9900 && token.balanceOf(vault) == 100, "withdrawal rollback");
        }
    }

    function testTokenCallbackCannotReenterDuringSettlement() public {
        token.setCallback(address(ledger), abi.encodeCall(ledger.createNamespace, (bytes32("reenter"))));
        vm.expectRevert(LedgerToken.TokenTransferFailed.selector);
        vm.prank(ALICE);
        ledger.deposit(a, 10);
        _balance(a, 0, 0);
        require(token.balanceOf(ALICE) == 10000 && token.balanceOf(vault) == 0, "rollback");
        token.setMode(TestToken.Mode.Normal);
        vm.prank(ALICE);
        ledger.deposit(a, 10);
        token.setMode(TestToken.Mode.Callback);
        vm.expectRevert(LedgerToken.TokenTransferFailed.selector);
        vm.prank(ALICE);
        ledger.withdraw(a, 10);
        _balance(a, 10, 0);
        require(token.balanceOf(vault) == 10, "withdrawal rollback");
    }

    function testSelfTransferChecksFundsAndZeroAmountStillChecksAuthority() public {
        vm.expectRevert(Ledgers.InsufficientBalance.selector);
        vm.prank(ALICE);
        ledger.transferClaims(a, a, 1);
        _unauthorized(ALICE);
        ledger.transferClaims(a, a, 0);
        vm.prank(ALICE);
        ledger.transferClaims(a, a, 0);
    }

    function testNoReturnTokensSettleExactlyAndMalformedReturnsRollBack() public {
        token.setMode(TestToken.Mode.NoReturn);
        vm.prank(ALICE);
        ledger.deposit(a, 20);
        vm.prank(ALICE);
        ledger.withdraw(a, 10);
        _balance(a, 10, 0);
        require(token.balanceOf(vault) == 10 && token.balanceOf(ALICE) == 9990, "no-return settlement");
        token.setMode(TestToken.Mode.MalformedReturn);
        vm.expectRevert(LedgerToken.TokenTransferFailed.selector);
        vm.prank(ALICE);
        ledger.deposit(a, 1);
        vm.expectRevert(LedgerToken.TokenTransferFailed.selector);
        vm.prank(ALICE);
        ledger.withdraw(a, 1);
        _balance(a, 10, 0);
        require(token.balanceOf(vault) == 10 && token.balanceOf(ALICE) == 9990, "malformed rollback");
    }

    function testFuzzOtherCallersCannotSpendClaims(address caller_, uint128 amount_) public {
        if (caller_ == ALICE) return;
        _unauthorized(ALICE);
        vm.prank(caller_);
        ledger.transferClaims(a, b, amount_);
        _balance(a, 0, 0);
        _balance(b, 0, 0);
    }

    function testCustodyBoundsAndDonationsNeverMintClaims() public {
        token.mint(ALICE, type(uint128).max);
        vm.prank(ALICE);
        ledger.deposit(a, type(uint128).max);
        vm.expectRevert(Ledgers.Overflow.selector);
        vm.prank(BOB);
        ledger.deposit(b, 1);
        vm.prank(ALICE);
        ledger.transferClaims(a, b, 1);
        _balance(asset, type(uint128).max, type(uint128).max);
        vm.prank(BOB);
        ledger.withdraw(b, 1);
        _balance(asset, type(uint128).max - 1, type(uint128).max - 1);
    }
}
