// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {TestBase, Ledgers, ControllerApp} from "./TestBase.sol";

contract ControllersTest is TestBase {
    function testZeroJournalPostingRequiresAndConsumesRecipientApproval() public {
        bytes32 operation_ = ledger.journalOperation(source, alice, 0);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 0);
        _approve(ALICE, operation_);
        ledger.postJournal(source, alice, 0);
        require(ledger.consents(ALICE, operation_) == 0, "zero still consumes consent");
        _balance(journal, 0, 0);
    }

    function testNamespacesAreIndependentAndRegistrationRequiresCreator() public {
        vm.prank(ALICE);
        bytes32 other_ = ledger.createNamespace(bytes32("test"));
        require(other_ != ns, "namespace isolation");
        _unauthorized(address(this));
        vm.prank(ALICE);
        ledger.createJournal(ns, bytes32("forged"));
        vm.prank(ALICE);
        bytes32 otherJournal_ = ledger.createJournal(other_, bytes32("journal"));
        require(otherJournal_ != journal, "root isolation");
        vm.expectRevert(abi.encodeWithSelector(Ledgers.AlreadyExists.selector, ns));
        ledger.createNamespace(bytes32("test"));
    }

    function testCreationNeedsParentAndNewControllerConsent() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), PARENT, Ledgers.Kind.DebitGroup);
        bytes32 operation_ = ledger.creationOperation(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        _unauthorized(ALICE);
        vm.prank(PARENT);
        ledger.createNode(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        _approve(ALICE, operation_);
        _unauthorized(PARENT);
        ledger.createNode(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        vm.prank(PARENT);
        bytes32 child_ = ledger.createNode(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        require(ledger.account(child_).controller == ALICE && ledger.account(group_).children == 1, "creation");
        require(ledger.consents(ALICE, operation_) == 0, "consumed");
        _unauthorized(ALICE);
        vm.prank(PARENT);
        ledger.closeNode(child_);
    }

    function testNeitherNamespaceNorParentCanPostForChildEvenZero() public {
        bytes32 group_ = _create(journal, journal, bytes32("group"), PARENT, Ledgers.Kind.DebitGroup);
        bytes32 child_ = _create(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        _post(source, child_, 20);
        for (uint128 amount_; amount_ < 2; ++amount_) {
            _unauthorized(ALICE);
            ledger.postJournal(child_, bob, amount_);
            _unauthorized(ALICE);
            vm.prank(PARENT);
            ledger.postJournal(child_, bob, amount_);
        }
        _unauthorized(ALICE);
        ledger.postJournal(child_, child_, 0);
        _balance(child_, 20, 0);
    }

    function testBothJournalEndpointsConsentAndApprovedActionIsSingleUse() public {
        bytes32 operation_ = ledger.journalOperation(source, alice, 10);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 10);
        _approve(ALICE, operation_);
        _unauthorized(BOB);
        ledger.postJournal(source, bob, 10);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 11);
        ledger.postJournal(source, alice, 10);
        _balance(alice, 10, 0);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 10);
    }

    function testRelayerRequiresBothApprovalsAndConsumesCallersOwnApproval() public {
        bytes32 operation_ = ledger.journalOperation(source, alice, 10);
        _approve(ALICE, operation_);
        _unauthorized(address(this));
        vm.prank(BOB);
        ledger.postJournal(source, alice, 10);
        _approve(address(this), operation_);
        // The caller's approval must also disappear, not become a standing delegated right.
        ledger.postJournal(source, alice, 10);
        require(ledger.consents(address(this), operation_) == 0 && ledger.consents(ALICE, operation_) == 0, "consumed");
        _approve(ALICE, operation_);
        _unauthorized(address(this));
        vm.prank(BOB);
        ledger.postJournal(source, alice, 10);
        _approve(address(this), operation_);
        vm.prank(BOB);
        ledger.postJournal(source, alice, 10);
        _balance(alice, 20, 0);
    }

    function testExpiredAndRevokedConsentCannotExecute() public {
        bytes32 operation_ = ledger.journalOperation(source, alice, 10);
        vm.warp(100);
        vm.prank(ALICE);
        ledger.approveOperation(operation_, 101);
        vm.warp(102);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 10);
        _approve(ALICE, operation_);
        vm.prank(ALICE);
        ledger.approveOperation(operation_, 0);
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 10);
        vm.expectRevert(Ledgers.InvalidDeadline.selector);
        ledger.approveOperation(operation_, 101);
    }

    function testConsentBoundToChainContractAndFullCreationParameters() public {
        bytes32 operation_ = ledger.creationOperation(journal, journal, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        _approve(ALICE, operation_);
        _unauthorized(ALICE);
        ledger.createNode(journal, journal, bytes32("child"), ALICE, Ledgers.Kind.CreditLeaf);
        _unauthorized(ALICE);
        ledger.createNode(journal, journal, bytes32("other"), ALICE, Ledgers.Kind.DebitLeaf);
        uint256 chain_ = block.chainid;
        vm.chainId(chain_ + 1);
        _unauthorized(ALICE);
        ledger.createNode(journal, journal, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        vm.chainId(chain_);
        Ledgers other_ = new Ledgers();
        bytes32 otherNs_ = other_.createNamespace(bytes32("test"));
        bytes32 otherJournal_ = other_.createJournal(otherNs_, bytes32("journal"));
        require(otherJournal_ == journal, "matching logical identities");
        vm.prank(ALICE);
        other_.approveOperation(operation_, type(uint64).max);
        _unauthorized(ALICE);
        other_.createNode(journal, journal, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        ledger.createNode(journal, journal, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
    }

    function testCloseReopenInvalidatesPendingPostingAndCreationConsent() public {
        bytes32 operation_ = ledger.journalOperation(source, alice, 10);
        _approve(ALICE, operation_);
        vm.prank(ALICE);
        ledger.closeNode(alice);
        bytes32 reopened_ = _create(journal, journal, bytes32("alice"), ALICE, Ledgers.Kind.DebitLeaf);
        require(reopened_ == alice && ledger.incarnations(alice) == 2, "incarnation");
        _unauthorized(ALICE);
        ledger.postJournal(source, alice, 10);

        bytes32 group_ = _create(journal, journal, bytes32("group"), PARENT, Ledgers.Kind.DebitGroup);
        bytes32 create_ = ledger.creationOperation(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
        _approve(ALICE, create_);
        vm.prank(PARENT);
        ledger.closeNode(group_);
        _create(journal, journal, bytes32("group"), PARENT, Ledgers.Kind.DebitGroup);
        _unauthorized(ALICE);
        vm.prank(PARENT);
        ledger.createNode(journal, group_, bytes32("child"), ALICE, Ledgers.Kind.DebitLeaf);
    }

    function testRevertingPostingRestoresApprovalAndBalances() public {
        bytes32 operation_ = ledger.journalOperation(alice, source, 1);
        _approve(address(this), operation_);
        vm.expectRevert(Ledgers.InsufficientBalance.selector);
        vm.prank(ALICE);
        ledger.postJournal(alice, source, 1);
        require(ledger.consents(address(this), operation_) != 0, "approval rollback");
        _balance(journal, 0, 0);
        _post(source, alice, 1);
        vm.prank(ALICE);
        ledger.postJournal(alice, source, 1);
        require(ledger.consents(address(this), operation_) == 0, "consumed after success");
    }

    function testContractControllersAuthenticateThroughCalls() public {
        ControllerApp app_ = new ControllerApp();
        bytes32 operation_ =
            ledger.creationOperation(journal, journal, bytes32("app"), address(app_), Ledgers.Kind.DebitLeaf);
        app_.execute(address(ledger), abi.encodeCall(ledger.approveOperation, (operation_, type(uint64).max)));
        bytes32 leaf_ = ledger.createNode(journal, journal, bytes32("app"), address(app_), Ledgers.Kind.DebitLeaf);
        app_.execute(
            address(ledger),
            abi.encodeCall(ledger.approveOperation, (ledger.journalOperation(source, leaf_, 20), type(uint64).max))
        );
        ledger.postJournal(source, leaf_, 20);
        _unauthorized(address(app_));
        ledger.transferJournal(leaf_, alice, 1);
        app_.execute(address(ledger), abi.encodeCall(ledger.transferJournal, (leaf_, alice, 7)));
        _balance(leaf_, 13, 0);
        _balance(alice, 7, 0);
    }
}
