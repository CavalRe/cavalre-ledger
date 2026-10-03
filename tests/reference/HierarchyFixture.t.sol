// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {ERC20TokenTest} from "contracts/tests/modules/ERC20Token.t.sol";
import {LedgerLib} from "contracts/modules/ledger/LedgerLib.sol";
import {TreeLib} from "contracts/modules/tree/TreeLib.sol";

/// Uses the upstream test adapter solely to expose authorized internal postings.
/// The posting implementation, wrapper, factory and tree are the pinned sources.
contract HierarchyFixtureTest is ERC20TokenTest {
    struct NodeSpec { uint256 parent; address relative; address absolute; bool credit; bool group; }
    NodeSpec[] private nodes;
    string private steps;
    uint256 private count;

    function add(uint256 parent, address relative, bool credit, bool group) private {
        address absolute;
        vm.prank(owner);
        if (group) {
            (absolute,) = ledgers.addSubAccountGroup(address(token), nodes[parent].absolute, relative, "Group", credit);
        } else {
            (absolute,) = ledgers.addSubAccount(address(token), nodes[parent].absolute, relative, "Leaf", credit);
        }
        nodes.push(NodeSpec(parent, relative, absolute, credit, group));
    }

    function snapshot() private view returns (uint256[] memory balances) {
        balances = new uint256[](nodes.length * 2);
        for (uint256 i; i < nodes.length; ++i) {
            TreeLib.TreeNode memory n = i == 0 ? tree.treeNode(address(token))
                : tree.treeNode(address(token), nodes[nodes[i].parent].absolute, nodes[i].relative);
            balances[2 * i] = n.debit;
            balances[2 * i + 1] = n.credit;
        }
        assertEq(balances[0], balances[1]);
        assertEq(balances[0], token.totalSupply());
        for (uint256 i; i < nodes.length; ++i) {
            if (!nodes[i].group) continue;
            uint256 d; uint256 c;
            for (uint256 j = 1; j < nodes.length; ++j) {
                if (nodes[j].parent == i) { d += balances[2*j]; c += balances[2*j+1]; }
            }
            assertEq(balances[2*i], d);
            assertEq(balances[2*i+1], c);
        }
    }

    function step(uint256 from, uint256 to, uint256 amount, bool publicTransfer) private {
        bool success;
        if (publicTransfer) {
            vm.prank(nodes[from].relative);
            (success,) = address(token).call(abi.encodeCall(token.transfer, (nodes[to].relative, amount)));
        } else {
            (success,) = address(ledgers).call(abi.encodeCall(ledgers.rawTransfer,
                (address(token), nodes[nodes[from].parent].absolute, nodes[from].relative,
                    nodes[nodes[to].parent].absolute, nodes[to].relative, amount)));
        }
        vm.serializeUint("hierarchy_step", "from", from);
        vm.serializeUint("hierarchy_step", "to", to);
        vm.serializeUint("hierarchy_step", "amount", amount);
        vm.serializeBool("hierarchy_step", "public", publicTransfer);
        vm.serializeBool("hierarchy_step", "success", success);
        string memory row = vm.serializeUint("hierarchy_step", "balances", snapshot());
        steps = string.concat(steps, count == 0 ? "" : ",", row); ++count;
    }

    function testExportHierarchyFixture() public {
        nodes.push(NodeSpec(0, address(token), address(token), false, true));
        nodes.push(NodeSpec(0, source_, LedgerLib.toAddress(address(token), source_), true, false));
        add(0, alice, false, false); // 2
        add(0, bob, false, false);   // 3
        add(0, address(0x101), false, true); // 4
        add(0, address(0x102), true, true);  // 5
        add(4, address(0x201), false, true); // 6
        add(5, address(0x202), true, true);  // 7
        add(4, alice, false, false); add(4, bob, true, false); // 8,9
        add(5, alice, false, false); add(5, bob, true, false); // 10,11
        add(6, alice, false, false); add(6, bob, true, false); // 12,13
        add(7, alice, false, false); add(7, bob, true, false); // 14,15
        string memory topology;
        for (uint256 i; i < nodes.length; ++i) {
            vm.serializeUint("node", "parent", nodes[i].parent);
            vm.serializeBool("node", "credit", nodes[i].credit);
            string memory row = vm.serializeBool("node", "group", nodes[i].group);
            topology = string.concat(topology, i == 0 ? "" : ",", row);
        }
        for (uint256 i = 2; i < nodes.length; ++i) {
            if (nodes[i].group) continue;
            if (nodes[i].credit) step(i, 2, 100, false);
            else step(1, i, 100, false);
        }
        // All 16 custodian/leaf polarity combinations, then unequal-depth paths.
        for (uint256 i = 8; i <= 11; ++i) {
            for (uint256 j = 8; j <= 11; ++j) step(i, j, 7, false);
        }
        step(8, 12, 7, false); step(9, 13, 7, false); step(9, 8, 7, false);
        step(2, 3, 10, true);
        step(2, 2, 1_000_000, true); // public self-transfer must check funds
        step(2, 2, 10, true);
        step(4, 3, 1, true); // presentation never grants custody-group spending
        step(1, 3, 1, true); // public source-credit spending is forbidden
        uint256[11] memory leaves = [uint256(1),2,3,8,9,10,11,12,13,14,15];
        uint256 seed = 0xCA7A1;
        for (uint256 i; i < 128; ++i) {
            seed = (seed * 1_664_525 + 1_013_904_223) & 0xffffffff;
            step(leaves[seed % 11], leaves[(seed >> 8) % 11], (seed >> 16) % 151, false);
        }
        vm.writeJson(string.concat(
            '{"schema_version":1,"contracts_commit":"34d159ff4e88fdfdee16738d9a1228f0bf407212",',
            '"nodes":[', topology, '],"steps":[', steps, "]}"
        ), "../../target/hierarchy-reference.json");
    }
}
