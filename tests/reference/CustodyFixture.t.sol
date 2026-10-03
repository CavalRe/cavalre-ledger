// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {Test} from "forge-std/src/Test.sol";
import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import {Dispatcher} from "contracts/modules/dispatcher/Dispatcher.sol";
import {Ledger} from "contracts/modules/ledger/Ledger.sol";
import {LedgerLib} from "contracts/modules/ledger/LedgerLib.sol";
import {LedgerView} from "contracts/modules/ledger/LedgerView.sol";
import {TreeLib} from "contracts/modules/tree/TreeLib.sol";
import {TreeView} from "contracts/modules/tree/TreeView.sol";

contract FixtureToken is ERC20 {
    constructor() ERC20("Custody fixture", "FIX") {}
    function mint(address to, uint256 amount) external { _mint(to, amount); }
}

/// Executes the pinned production Ledger through its real Dispatcher. The only
/// model-specific adapter is ERC20 balances <-> SPL raw token amounts.
contract CustodyFixtureTest is Test {
    Ledger private ledger;
    LedgerView private view_;
    TreeView private tree;
    FixtureToken private token;
    address[2] private users = [address(0xA11CE), address(0xB0B)];
    string private steps;
    uint256 private stepCount;
    uint256 private failures;

    function setUp() public {
        Dispatcher dispatcher = new Dispatcher(address(this));
        address[] memory modules = new address[](3);
        modules[0] = address(new Ledger(18, "Ether", "ETH", 18));
        modules[1] = address(new LedgerView());
        modules[2] = address(new TreeView());
        dispatcher.addModule(modules);
        ledger = Ledger(payable(address(dispatcher)));
        view_ = LedgerView(address(dispatcher));
        tree = TreeView(address(dispatcher));
        ledger.initializeLedger("Custody", "C");
        token = new FixtureToken();
        address[] memory tokens = new address[](1);
        tokens[0] = address(token);
        ledger.addExternalToken(tokens);
        ledger.addSubAccount(address(token), address(token), LedgerLib.SOURCE_ADDRESS, "Source", true);
        for (uint256 i; i < 2; ++i) {
            token.mint(users[i], 10_000);
            vm.prank(users[i]);
            token.approve(address(ledger), type(uint256).max);
        }
    }

    // kind: 0 = deposit; 1 = withdraw; 2 = direct donation. Capture the observed
    // success/revert and all economic balances, including after failed calls.
    function step(uint256 kind, uint256 user, uint256 amount) private {
        vm.prank(users[user]);
        bool success;
        if (kind == 2) {
            (success,) = address(token).call(abi.encodeCall(token.transfer, (address(ledger), amount)));
        } else if (kind == 0) {
            (success,) = address(ledger).call(abi.encodeCall(ledger.wrap, (address(token), amount)));
        } else {
            (success,) = address(ledger).call(abi.encodeCall(ledger.unwrap, (address(token), amount)));
        }
        if (!success) ++failures;
        uint256[] memory positions = new uint256[](2);
        uint256[] memory wallets = new uint256[](2);
        for (uint256 i; i < 2; ++i) {
            positions[i] = view_.debitBalanceOf(address(token), address(token), users[i]);
            wallets[i] = token.balanceOf(users[i]);
        }
        uint256 supply = view_.totalSupply(address(token));
        uint256 collateral = token.balanceOf(address(ledger));
        TreeLib.TreeNode memory root = tree.treeNode(address(token));
        assertEq(root.debit, root.credit);
        assertEq(root.debit, supply);
        assertEq(supply, positions[0] + positions[1]);
        assertGe(collateral, supply);
        assertEq(collateral + wallets[0] + wallets[1], 20_000);
        vm.serializeUint("step", "kind", kind);
        vm.serializeUint("step", "user", user);
        vm.serializeUint("step", "amount", amount);
        vm.serializeBool("step", "success", success);
        vm.serializeUint("step", "positions", positions);
        vm.serializeUint("step", "wallets", wallets);
        vm.serializeUint("step", "total_claims", supply);
        string memory row = vm.serializeUint("step", "vault", collateral);
        steps = string.concat(steps, stepCount == 0 ? "" : ",", row);
        ++stepCount;
    }

    function testExportCustodyFixture() public {
        step(0, 0, 120);
        step(0, 1, 80);
        step(1, 0, 45);
        step(2, 1, 9);
        step(1, 1, 81); // overdraft
        step(0, 0, 0);
        step(1, 1, 0);
        step(0, 0, 10_001); // inadequate external balance
        step(1, 0, 75);
        step(1, 1, 80);
        assertEq(failures, 2);
        uint256 seed = 0xCA7A1;
        for (uint256 i; i < 128; ++i) {
            seed = (seed * 1_664_525 + 1_013_904_223) & 0xffffffff;
            uint256 kind = i % 3;
            step(kind, (seed >> 8) % 2, seed % (kind == 2 ? 21 : 2_000));
        }
        assertGt(failures, 2);
        string memory output = string.concat(
            '{"schema_version":1,"contracts_commit":"34d159ff4e88fdfdee16738d9a1228f0bf407212",',
            '"initial_tokens_per_user":10000,"steps":[', steps, "]}"
        );
        vm.writeJson(output, "../../target/custody-reference.json");
    }
}
