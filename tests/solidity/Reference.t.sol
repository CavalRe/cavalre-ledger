// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {TestBase, Ledgers, TestToken} from "./TestBase.sol";

/// @dev JSON struct fields follow Foundry's alphabetical key ordering.
contract ReferenceTest is TestBase {
    struct NodeSpec {
        bool credit;
        bool group;
        uint256 parent;
    }

    struct HierarchyStep {
        uint256 amount;
        uint256[] balances;
        uint256 from;
        bool publicTransfer;
        bool success;
        uint256 to;
    }

    struct CustodyStep {
        uint256 amount;
        uint256 kind;
        uint256[] positions;
        bool success;
        uint256 totalClaims;
        uint256 user;
        uint256 vault;
        uint256[] wallets;
    }

    struct CustodyContext {
        TestToken token;
        bytes32 root;
        bytes32[2] positions;
        address[2] users;
        address vault;
    }

    function _fixture(string memory path_) private view returns (string memory json_) {
        json_ = vm.readFile(path_);
        string memory pin_ = vm.readFile("spec/upstream.json");
        require(
            keccak256(bytes(vm.parseJsonString(json_, ".contracts_commit")))
                == keccak256(bytes(vm.parseJsonString(pin_, ".references[0].commit"))),
            "reference pin"
        );
        require(vm.parseJsonUint(json_, ".schema_version") == 1, "schema");
    }

    function testReplaysAll162HierarchyActions() public {
        string memory json_ = _fixture("spec/fixtures/hierarchy.json");
        NodeSpec[] memory specs_ = abi.decode(vm.parseJson(json_, ".nodes"), (NodeSpec[]));
        HierarchyStep[] memory steps_ = abi.decode(vm.parseJson(json_, ".steps"), (HierarchyStep[]));
        require(steps_.length == 162, "all hierarchy steps");
        bytes32[] memory nodes_ = new bytes32[](specs_.length);
        nodes_[0] = ledger.createJournal(ns, bytes32("fixture"));
        for (uint256 i_ = 1; i_ < specs_.length; ++i_) {
            Ledgers.Kind kind_ = specs_[i_].group
                ? (specs_[i_].credit ? Ledgers.Kind.CreditGroup : Ledgers.Kind.DebitGroup)
                : (specs_[i_].credit ? Ledgers.Kind.CreditLeaf : Ledgers.Kind.DebitLeaf);
            nodes_[i_] = ledger.createNode(nodes_[0], nodes_[specs_[i_].parent], bytes32(i_), address(this), kind_);
        }
        uint256 failures_;
        for (uint256 i_; i_ < steps_.length; ++i_) {
            HierarchyStep memory step_ = steps_[i_];
            require(step_.amount <= type(uint128).max, "fixture amount range");
            bytes memory data_ = step_.publicTransfer
                ? abi.encodeCall(ledger.transferJournal, (nodes_[step_.from], nodes_[step_.to], uint128(step_.amount)))
                : abi.encodeCall(ledger.postJournal, (nodes_[step_.from], nodes_[step_.to], uint128(step_.amount)));
            (bool success_,) = address(ledger).call(data_);
            require(success_ == step_.success, "hierarchy result");
            if (!success_) ++failures_;
            for (uint256 n_; n_ < nodes_.length; ++n_) {
                Ledgers.Balances memory actual_ = ledger.balances(nodes_[n_]);
                require(actual_.debit == step_.balances[n_ * 2], "fixture debit");
                require(actual_.credit == step_.balances[n_ * 2 + 1], "fixture credit");
                if (specs_[n_].group) {
                    uint256 debit_;
                    uint256 credit_;
                    for (uint256 child_ = 1; child_ < nodes_.length; ++child_) {
                        if (specs_[child_].parent == n_) {
                            Ledgers.Balances memory childBalance_ = ledger.balances(nodes_[child_]);
                            debit_ += childBalance_.debit;
                            credit_ += childBalance_.credit;
                        }
                    }
                    require(debit_ == actual_.debit && credit_ == actual_.credit, "independent aggregate");
                }
            }
        }
        require(failures_ > 3, "rejection coverage");
    }

    function testReplaysAll138CustodyActions() public {
        string memory json_ = _fixture("spec/fixtures/custody.json");
        CustodyStep[] memory steps_ = abi.decode(vm.parseJson(json_, ".steps"), (CustodyStep[]));
        require(steps_.length == 138 && vm.parseJsonUint(json_, ".initial_tokens_per_user") == 10000, "custody fixture");
        CustodyContext memory ctx;
        (ctx.token, ctx.root, ctx.positions[0], ctx.positions[1]) = _asset();
        ctx.users = [ALICE, BOB];
        ctx.vault = ledger.root(ctx.root).vault;
        uint256 failures_;
        for (uint256 i_; i_ < steps_.length; ++i_) {
            CustodyStep memory step_ = steps_[i_];
            require(step_.amount <= type(uint128).max, "fixture amount range");
            address target_ = address(ledger);
            bytes memory data_;
            if (step_.kind == 0) {
                data_ = abi.encodeCall(ledger.deposit, (ctx.positions[step_.user], uint128(step_.amount)));
            } else if (step_.kind == 1) {
                data_ = abi.encodeCall(ledger.withdraw, (ctx.positions[step_.user], uint128(step_.amount)));
            } else {
                require(step_.kind == 2, "action kind");
                target_ = address(ctx.token);
                data_ = abi.encodeCall(ctx.token.transfer, (ctx.vault, step_.amount));
            }
            vm.prank(ctx.users[step_.user]);
            (bool success_,) = target_.call(data_);
            require(success_ == step_.success, "custody result");
            if (!success_) ++failures_;
            _assertCustodyState(ctx, step_);
        }
        require(failures_ > 2, "rejection coverage");
    }

    function _assertCustodyState(CustodyContext memory ctx, CustodyStep memory step_) private view {
        uint256 claims_;
        uint256 wallets_;
        for (uint256 user_; user_ < 2; ++user_) {
            Ledgers.Balances memory actual_ = ledger.balances(ctx.positions[user_]);
            require(actual_.debit == step_.positions[user_] && actual_.credit == 0, "fixture position");
            uint256 wallet_ = ctx.token.balanceOf(ctx.users[user_]);
            require(wallet_ == step_.wallets[user_], "fixture wallet");
            claims_ += actual_.debit;
            wallets_ += wallet_;
        }
        Ledgers.Balances memory gross_ = ledger.balances(ctx.root);
        require(gross_.debit == step_.totalClaims && gross_.credit == step_.totalClaims, "fixture gross");
        uint256 collateral_ = ctx.token.balanceOf(ctx.vault);
        require(collateral_ == step_.vault, "fixture vault");
        require(claims_ == gross_.debit && collateral_ >= claims_, "solvency");
        require(collateral_ + wallets_ == 20000, "token conservation");
    }
}
