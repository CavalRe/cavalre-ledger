// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {Ledgers} from "../../contracts/Ledgers.sol";
import {LedgerVault, LedgerToken} from "../../contracts/LedgerVault.sol";

interface Vm {
    function prank(address) external;
    function expectRevert(bytes calldata) external;
    function expectRevert(bytes4) external;
    function warp(uint256) external;
    function chainId(uint256) external;
    function readFile(string calldata) external view returns (string memory);
    function parseJson(string calldata, string calldata) external pure returns (bytes memory);
    function parseJsonString(string calldata, string calldata) external pure returns (string memory);
    function parseJsonUint(string calldata, string calldata) external pure returns (uint256);
}

/// @dev Deliberately configurable token for adversarial settlement tests, never a deployment target.
contract TestToken {
    enum Mode {
        Normal,
        RecipientFee,
        SenderFee,
        NoMovement,
        ReturnFalse,
        Callback,
        NoReturn,
        MalformedReturn
    }
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    Mode public mode;
    address public callback;
    bytes public callbackData;

    function mint(address to_, uint256 amount_) external {
        balanceOf[to_] += amount_;
    }

    function burn(address from_, uint256 amount_) external {
        balanceOf[from_] -= amount_;
    }

    function setMode(Mode mode_) external {
        mode = mode_;
    }

    function setCallback(address callback_, bytes calldata data_) external {
        callback = callback_;
        callbackData = data_;
        mode = Mode.Callback;
    }

    function approve(address spender_, uint256 amount_) external returns (bool) {
        allowance[msg.sender][spender_] = amount_;
        return true;
    }

    function transfer(address to_, uint256 amount_) external returns (bool) {
        return _move(msg.sender, to_, amount_);
    }

    function transferFrom(address from_, address to_, uint256 amount_) external returns (bool) {
        allowance[from_][msg.sender] -= amount_;
        return _move(from_, to_, amount_);
    }

    function _move(address from_, address to_, uint256 amount_) private returns (bool) {
        if (mode == Mode.ReturnFalse) return false;
        if (mode == Mode.NoMovement) return true;
        balanceOf[from_] -= amount_ + (mode == Mode.SenderFee ? 1 : 0);
        balanceOf[to_] += amount_ - (mode == Mode.RecipientFee ? 1 : 0);
        if (mode == Mode.Callback) {
            (bool success_,) = callback.call(callbackData);
            require(success_, "callback rejected");
        }
        if (mode == Mode.NoReturn) {
            assembly { return(0, 0) }
        }
        if (mode == Mode.MalformedReturn) {
            assembly { return(0, 1) }
        }
        return true;
    }
}

contract ControllerApp {
    address private immutable _owner = msg.sender;

    function execute(address target_, bytes calldata data_) external returns (bytes memory) {
        require(msg.sender == _owner, "owner");
        (bool success_, bytes memory result_) = target_.call(data_);
        if (!success_) {
            assembly { revert(add(result_, 32), mload(result_)) }
        }
        return result_;
    }
}

abstract contract TestBase {
    Vm internal constant vm = Vm(address(uint160(uint256(keccak256("hevm cheat code")))));
    address internal constant ALICE = address(0xA11CE);
    address internal constant BOB = address(0xB0B);
    address internal constant PARENT = address(0x1234);
    Ledgers internal ledger;
    bytes32 internal ns;
    bytes32 internal journal;
    bytes32 internal source;
    bytes32 internal alice;
    bytes32 internal bob;

    function setUp() public virtual {
        ledger = new Ledgers();
        ns = ledger.createNamespace(bytes32("test"));
        journal = ledger.createJournal(ns, bytes32("journal"));
        source = _create(journal, journal, bytes32("source"), address(this), Ledgers.Kind.CreditLeaf);
        alice = _create(journal, journal, bytes32("alice"), ALICE, Ledgers.Kind.DebitLeaf);
        bob = _create(journal, journal, bytes32("bob"), BOB, Ledgers.Kind.DebitLeaf);
    }

    function _approve(address controller_, bytes32 operation_) internal {
        vm.prank(controller_);
        ledger.approveOperation(operation_, type(uint64).max);
    }

    function _create(bytes32 root_, bytes32 parent_, bytes32 relative_, address controller_, Ledgers.Kind kind_)
        internal
        returns (bytes32)
    {
        address structural_ =
            parent_ == root_ ? ledger.namespaces(ledger.root(root_).namespace) : ledger.account(parent_).controller;
        if (controller_ != structural_) {
            _approve(controller_, ledger.creationOperation(root_, parent_, relative_, controller_, kind_));
        }
        vm.prank(structural_);
        return ledger.createNode(root_, parent_, relative_, controller_, kind_);
    }

    function _post(bytes32 from_, bytes32 to_, uint128 amount_) internal {
        address fromController_ = ledger.account(from_).controller;
        address toController_ = ledger.account(to_).controller;
        if (fromController_ != toController_) _approve(toController_, ledger.journalOperation(from_, to_, amount_));
        vm.prank(fromController_);
        ledger.postJournal(from_, to_, amount_);
    }

    function _asset() internal returns (TestToken token_, bytes32 root_, bytes32 a_, bytes32 b_) {
        token_ = new TestToken();
        root_ = ledger.registerAsset(ns, address(token_));
        vm.prank(ALICE);
        a_ = ledger.openPosition(root_);
        vm.prank(BOB);
        b_ = ledger.openPosition(root_);
        token_.mint(ALICE, 10000);
        token_.mint(BOB, 10000);
        vm.prank(ALICE);
        token_.approve(address(ledger), type(uint256).max);
        vm.prank(BOB);
        token_.approve(address(ledger), type(uint256).max);
    }

    function _balance(bytes32 id_, uint128 debit_, uint128 credit_) internal view {
        Ledgers.Balances memory balance_ = ledger.balances(id_);
        require(balance_.debit == debit_ && balance_.credit == credit_, "balances");
    }

    function _unauthorized(address controller_) internal {
        vm.expectRevert(abi.encodeWithSelector(Ledgers.Unauthorized.selector, controller_));
    }
}
