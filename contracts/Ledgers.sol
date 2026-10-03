// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

import {ILedgerToken, LedgerToken, LedgerVault} from "./LedgerVault.sol";

/// @notice Independent hierarchical accounting and ERC20 custody with per-account controllers.
/// @dev Controllers are authenticated callers. Exact-operation consent handles the Rust core's
/// multi-signer operations. There is no administrator with inherited spending authority.
contract Ledgers {
    enum RootKind {
        Unregistered,
        Asset,
        Journal
    }

    enum Kind {
        DebitLeaf,
        CreditLeaf,
        DebitGroup,
        CreditGroup
    }

    enum Operation {
        PostJournal,
        TransferJournal,
        TransferClaims
    }

    struct Balances {
        uint128 debit;
        uint128 credit;
    }

    struct Root {
        bytes32 namespace;
        RootKind kind;
        address token;
        address vault;
        uint128 gross;
    }

    struct Account {
        bytes32 root;
        bytes32 parent;
        address controller;
        Kind kind;
        uint8 depth;
        uint32 children;
        Balances balances;
    }

    error Unauthorized(address controller);
    error AlreadyExists(bytes32 id);
    error InvalidAccount(bytes32 id);
    error InvalidController();
    error InvalidDeadline();
    error UnsupportedRoot();
    error DifferentRoots();
    error DepthLimit();
    error NonemptyAccount();
    error InsufficientBalance();
    error Overflow();
    error Undercollateralized();
    error UnexpectedTokenDelta();
    error Reentrancy();

    event NamespaceCreated(bytes32 indexed namespace, address indexed controller, bytes32 relative);
    event RootCreated(bytes32 indexed root, bytes32 indexed namespace, RootKind kind, address token, address vault);
    event AccountCreated(
        bytes32 indexed account,
        bytes32 indexed root,
        bytes32 indexed parent,
        bytes32 relative,
        address controller,
        Kind kind,
        uint64 incarnation
    );
    event AccountClosed(bytes32 indexed account, uint64 incarnation);
    event ConsentChanged(address indexed controller, bytes32 indexed operation, uint64 deadline);
    event ConsentConsumed(address indexed controller, bytes32 indexed operation);
    event Posted(bytes32 indexed root, bytes32 indexed from, bytes32 indexed to, uint128 amount, Operation operation);
    event Deposited(bytes32 indexed root, bytes32 indexed account, uint128 amount);
    event Withdrawn(bytes32 indexed root, bytes32 indexed account, uint128 amount);

    uint8 public constant MAX_DEPTH = 16;

    mapping(bytes32 => address) public namespaces;
    mapping(bytes32 => Root) private _roots;
    mapping(bytes32 => Account) private _accounts;
    // Retained after closure so pending approvals cannot authorize a replacement account.
    mapping(bytes32 => uint64) public incarnations;
    mapping(address => mapping(bytes32 => uint64)) public consents;
    uint256 private _entered = 1;

    modifier nonReentrant() {
        if (_entered != 1) revert Reentrancy();
        _entered = 2;
        _;
        _entered = 1;
    }

    // -- Identity and views --

    function namespaceId(address controller_, bytes32 relative_) public pure returns (bytes32) {
        return keccak256(abi.encode("namespace", controller_, relative_));
    }

    function journalId(bytes32 namespace_, bytes32 relative_) public pure returns (bytes32) {
        return keccak256(abi.encode("journal", namespace_, relative_));
    }

    function assetId(bytes32 namespace_, address token_) public pure returns (bytes32) {
        return keccak256(abi.encode("asset", namespace_, token_));
    }

    function accountId(bytes32 root_, bytes32 parent_, bytes32 relative_) public pure returns (bytes32) {
        return keccak256(abi.encode("account", root_, parent_, relative_));
    }

    function root(bytes32 root_) external view returns (Root memory) {
        return _enforceRoot(root_);
    }

    function account(bytes32 account_) external view returns (Account memory) {
        return _enforceAccount(account_);
    }

    /// @notice Root debit and credit derive from the same authoritative gross supply.
    function balances(bytes32 id_) external view returns (Balances memory) {
        if (_roots[id_].kind != RootKind.Unregistered) {
            uint128 gross_ = _roots[id_].gross;
            return Balances(gross_, gross_);
        }
        return _enforceAccount(id_).balances;
    }

    // -- Exact-operation consent --

    /// @notice Approve one exact operation until deadline; zero revokes it.
    /// @dev An approval grants no general delegate or subtree authority.
    function approveOperation(bytes32 operation_, uint64 deadline_) external nonReentrant {
        if (deadline_ != 0 && deadline_ < block.timestamp) revert InvalidDeadline();
        consents[msg.sender][operation_] = deadline_;
        emit ConsentChanged(msg.sender, operation_, deadline_);
    }

    function creationOperation(bytes32 root_, bytes32 parent_, bytes32 relative_, address controller_, Kind kind_)
        public
        view
        returns (bytes32)
    {
        bytes32 id_ = accountId(root_, parent_, relative_);
        return keccak256(
            abi.encode(
                block.chainid,
                address(this),
                this.createNode.selector,
                root_,
                parent_,
                relative_,
                controller_,
                kind_,
                incarnations[parent_],
                incarnations[id_] + 1
            )
        );
    }

    function journalOperation(bytes32 from_, bytes32 to_, uint128 amount_) public view returns (bytes32) {
        return keccak256(
            abi.encode(
                block.chainid,
                address(this),
                this.postJournal.selector,
                from_,
                to_,
                amount_,
                incarnations[from_],
                incarnations[to_]
            )
        );
    }

    function _consumeConsent(address controller_, bytes32 operation_) private {
        uint64 deadline_ = consents[controller_][operation_];
        if (msg.sender != controller_ && (deadline_ == 0 || deadline_ < block.timestamp)) {
            revert Unauthorized(controller_);
        }
        // Consume even the caller's existing approval to prevent its later reuse by a relayer.
        if (deadline_ != 0) {
            delete consents[controller_][operation_];
            emit ConsentConsumed(controller_, operation_);
        }
    }

    // -- Namespace, root and account lifecycle --

    function createNamespace(bytes32 relative_) external nonReentrant returns (bytes32 id_) {
        id_ = namespaceId(msg.sender, relative_);
        if (namespaces[id_] != address(0)) revert AlreadyExists(id_);
        namespaces[id_] = msg.sender;
        emit NamespaceCreated(id_, msg.sender, relative_);
    }

    function createJournal(bytes32 namespace_, bytes32 relative_) external nonReentrant returns (bytes32 id_) {
        _enforceController(namespaces[namespace_]);
        id_ = journalId(namespace_, relative_);
        if (_roots[id_].kind != RootKind.Unregistered) revert AlreadyExists(id_);
        _roots[id_] = Root(namespace_, RootKind.Journal, address(0), address(0), 0);
        emit RootCreated(id_, namespace_, RootKind.Journal, address(0), address(0));
    }

    /// @notice Register one isolated ERC20 custody vault for this namespace and asset.
    function registerAsset(bytes32 namespace_, address token_) external nonReentrant returns (bytes32 id_) {
        _enforceController(namespaces[namespace_]);
        if (token_.code.length == 0) revert UnsupportedRoot();
        id_ = assetId(namespace_, token_);
        if (_roots[id_].kind != RootKind.Unregistered) revert AlreadyExists(id_);
        address vault_ = address(new LedgerVault(token_));
        _roots[id_] = Root(namespace_, RootKind.Asset, token_, vault_, 0);
        emit RootCreated(id_, namespace_, RootKind.Asset, token_, vault_);
    }

    function createNode(bytes32 root_, bytes32 parent_, bytes32 relative_, address controller_, Kind kind_)
        external
        nonReentrant
        returns (bytes32 id_)
    {
        Root storage rootData_ = _enforceRoot(root_);
        if (controller_ == address(0)) revert InvalidController();
        if (rootData_.kind == RootKind.Asset && kind_ == Kind.CreditLeaf) revert UnsupportedRoot();
        address structural_ = namespaces[rootData_.namespace];
        uint8 depth_ = 1;
        if (parent_ != root_) {
            Account storage parentData_ = _enforceAccount(parent_);
            if (parentData_.root != root_ || !_isGroup(parentData_.kind)) revert InvalidAccount(parent_);
            if (parentData_.depth == MAX_DEPTH) revert DepthLimit();
            structural_ = parentData_.controller;
            depth_ = parentData_.depth + 1;
        }
        bytes32 operation_ = creationOperation(root_, parent_, relative_, controller_, kind_);
        _consumeConsent(structural_, operation_);
        if (controller_ != structural_) _consumeConsent(controller_, operation_);
        id_ = _createNode(root_, parent_, relative_, controller_, kind_, depth_);
    }

    /// @notice Open the caller's direct Asset position without namespace consent.
    function openPosition(bytes32 root_) external nonReentrant returns (bytes32) {
        if (_enforceRoot(root_).kind != RootKind.Asset) revert UnsupportedRoot();
        return _createNode(root_, root_, bytes32(uint256(uint160(msg.sender))), msg.sender, Kind.DebitLeaf, 1);
    }

    function _createNode(
        bytes32 root_,
        bytes32 parent_,
        bytes32 relative_,
        address controller_,
        Kind kind_,
        uint8 depth_
    ) private returns (bytes32 id_) {
        id_ = accountId(root_, parent_, relative_);
        if (_accounts[id_].controller != address(0)) revert AlreadyExists(id_);
        uint64 incarnation_ = ++incarnations[id_];
        _accounts[id_] = Account(root_, parent_, controller_, kind_, depth_, 0, Balances(0, 0));
        if (parent_ != root_) ++_accounts[parent_].children;
        emit AccountCreated(id_, root_, parent_, relative_, controller_, kind_, incarnation_);
    }

    function closeNode(bytes32 account_) external nonReentrant {
        Account storage node_ = _enforceAccount(account_);
        _enforceController(node_.controller);
        if (node_.balances.debit != 0 || node_.balances.credit != 0 || node_.children != 0) {
            revert NonemptyAccount();
        }
        if (node_.parent != node_.root) --_accounts[node_.parent].children;
        delete _accounts[account_];
        emit AccountClosed(account_, incarnations[account_]);
    }

    // -- Controller-authorized postings --

    function postJournal(bytes32 from_, bytes32 to_, uint128 amount_) external nonReentrant {
        _enforceEndpoints(from_, to_, RootKind.Journal);
        bytes32 operation_ = journalOperation(from_, to_, amount_);
        address controller_ = _accounts[from_].controller;
        _consumeConsent(controller_, operation_);
        if (_accounts[to_].controller != controller_) _consumeConsent(_accounts[to_].controller, operation_);
        _post(from_, to_, amount_, Operation.PostJournal);
    }

    function transferJournal(bytes32 from_, bytes32 to_, uint128 amount_) external nonReentrant {
        _enforceEndpoints(from_, to_, RootKind.Journal);
        Account storage fromData_ = _accounts[from_];
        Account storage toData_ = _accounts[to_];
        if (
            fromData_.depth != 1 || toData_.depth != 1 || fromData_.kind != Kind.DebitLeaf
                || toData_.kind != Kind.DebitLeaf
        ) revert InvalidAccount(from_);
        _enforceController(fromData_.controller);
        if (fromData_.balances.debit < amount_) revert InsufficientBalance();
        _post(from_, to_, amount_, Operation.TransferJournal);
    }

    function transferClaims(bytes32 from_, bytes32 to_, uint128 amount_) external nonReentrant {
        _enforceEndpoints(from_, to_, RootKind.Asset);
        _enforceController(_accounts[from_].controller);
        if (_accounts[from_].balances.debit < amount_) revert InsufficientBalance();
        _post(from_, to_, amount_, Operation.TransferClaims);
    }

    function _post(bytes32 from_, bytes32 to_, uint128 amount_, Operation operation_) private {
        bytes32 root_ = _accounts[from_].root;
        if (from_ != to_ && amount_ != 0) {
            bool fromCredit_ = _accounts[from_].kind == Kind.CreditLeaf;
            bool toCredit_ = _accounts[to_].kind == Kind.CreditLeaf;
            bytes32[] memory fromPath_ = _path(from_);
            bytes32[] memory toPath_ = _path(to_);
            for (uint256 i_; i_ < fromPath_.length; ++i_) {
                bool shared_ = _contains(toPath_, fromPath_[i_]);
                // Same-side shared ancestors cancel before bounded arithmetic.
                if (shared_ && fromCredit_ == toCredit_) continue;
                _change(fromPath_[i_], true, shared_, fromCredit_, toCredit_, amount_);
            }
            for (uint256 i_; i_ < toPath_.length; ++i_) {
                if (!_contains(fromPath_, toPath_[i_])) {
                    _change(toPath_[i_], false, true, fromCredit_, toCredit_, amount_);
                }
            }
            if (fromCredit_ != toCredit_) {
                uint128 gross_ = _roots[root_].gross;
                _roots[root_].gross = fromCredit_ ? _add(gross_, amount_) : _subtract(gross_, amount_);
            }
        }
        emit Posted(root_, from_, to_, amount_, operation_);
    }

    function _change(bytes32 id_, bool from_, bool to_, bool fromCredit_, bool toCredit_, uint128 amount_) private {
        Balances memory next_ = _accounts[id_].balances;
        if (from_) {
            if (fromCredit_) next_.credit = _add(next_.credit, amount_);
            else next_.debit = _subtract(next_.debit, amount_);
        }
        if (to_) {
            if (toCredit_) next_.credit = _subtract(next_.credit, amount_);
            else next_.debit = _add(next_.debit, amount_);
        }
        _accounts[id_].balances = next_;
    }

    /// @dev Paths come only from immutable authenticated contract storage, never caller snapshots.
    function _path(bytes32 id_) private view returns (bytes32[] memory path_) {
        path_ = new bytes32[](_accounts[id_].depth);
        for (uint256 i_; i_ < path_.length; ++i_) {
            path_[i_] = id_;
            id_ = _accounts[id_].parent;
        }
    }

    function _contains(bytes32[] memory path_, bytes32 id_) private pure returns (bool) {
        for (uint256 i_; i_ < path_.length; ++i_) {
            if (path_[i_] == id_) return true;
        }
        return false;
    }

    // -- Exact ERC20 custody --

    function deposit(bytes32 position_, uint128 amount_) external nonReentrant {
        Account storage node_ = _enforcePosition(position_);
        Root storage root_ = _roots[node_.root];
        uint128 balance_ = _add(node_.balances.debit, amount_);
        uint128 gross_ = _add(root_.gross, amount_);
        ILedgerToken token_ = ILedgerToken(root_.token);
        uint256 vaultBefore_ = token_.balanceOf(root_.vault);
        uint256 walletBefore_ = token_.balanceOf(msg.sender);
        LedgerToken.safeTransferCall(
            root_.token, abi.encodeCall(ILedgerToken.transferFrom, (msg.sender, root_.vault, amount_))
        );
        _enforceDelta(vaultBefore_, token_.balanceOf(root_.vault), amount_, true);
        _enforceDelta(walletBefore_, token_.balanceOf(msg.sender), amount_, false);
        // Issue claims only after observing exact receipt and exact sender debit.
        node_.balances.debit = balance_;
        root_.gross = gross_;
        emit Deposited(node_.root, position_, amount_);
    }

    function withdraw(bytes32 position_, uint128 amount_) external nonReentrant {
        Account storage node_ = _enforcePosition(position_);
        Root storage root_ = _roots[node_.root];
        ILedgerToken token_ = ILedgerToken(root_.token);
        uint256 vaultBefore_ = token_.balanceOf(root_.vault);
        uint256 walletBefore_ = token_.balanceOf(msg.sender);
        // Check all outstanding liabilities, including for a zero or small withdrawal.
        if (vaultBefore_ < root_.gross) revert Undercollateralized();
        node_.balances.debit = _subtract(node_.balances.debit, amount_);
        root_.gross = _subtract(root_.gross, amount_);
        LedgerVault(root_.vault).send(msg.sender, amount_);
        _enforceDelta(vaultBefore_, token_.balanceOf(root_.vault), amount_, false);
        _enforceDelta(walletBefore_, token_.balanceOf(msg.sender), amount_, true);
        // Any failed native transfer or delta check reverts the claims and token movement together.
        emit Withdrawn(node_.root, position_, amount_);
    }

    // -- Validation and checked arithmetic --

    function _enforceController(address controller_) private view {
        if (controller_ == address(0) || msg.sender != controller_) revert Unauthorized(controller_);
    }

    function _enforceRoot(bytes32 id_) private view returns (Root storage root_) {
        root_ = _roots[id_];
        if (root_.kind == RootKind.Unregistered) revert UnsupportedRoot();
    }

    function _enforceAccount(bytes32 id_) private view returns (Account storage node_) {
        node_ = _accounts[id_];
        if (node_.controller == address(0)) revert InvalidAccount(id_);
    }

    function _enforceEndpoints(bytes32 from_, bytes32 to_, RootKind kind_) private view {
        Account storage fromData_ = _enforceAccount(from_);
        Account storage toData_ = _enforceAccount(to_);
        if (fromData_.root != toData_.root) revert DifferentRoots();
        if (_roots[fromData_.root].kind != kind_) revert UnsupportedRoot();
        if (_isGroup(fromData_.kind)) revert InvalidAccount(from_);
        if (_isGroup(toData_.kind)) revert InvalidAccount(to_);
    }

    function _enforcePosition(bytes32 id_) private view returns (Account storage node_) {
        node_ = _enforceAccount(id_);
        if (_roots[node_.root].kind != RootKind.Asset) revert UnsupportedRoot();
        if (node_.depth != 1 || node_.kind != Kind.DebitLeaf) revert InvalidAccount(id_);
        _enforceController(node_.controller);
    }

    function _enforceDelta(uint256 before_, uint256 after_, uint128 amount_, bool increase_) private pure {
        if (increase_) {
            if (after_ < before_ || after_ - before_ != amount_) revert UnexpectedTokenDelta();
        } else {
            if (before_ < after_ || before_ - after_ != amount_) revert UnexpectedTokenDelta();
        }
    }

    function _isGroup(Kind kind_) private pure returns (bool) {
        return kind_ == Kind.DebitGroup || kind_ == Kind.CreditGroup;
    }

    function _add(uint128 balance_, uint128 amount_) private pure returns (uint128) {
        if (amount_ > type(uint128).max - balance_) revert Overflow();
        return balance_ + amount_;
    }

    function _subtract(uint128 balance_, uint128 amount_) private pure returns (uint128) {
        if (balance_ < amount_) revert InsufficientBalance();
        return balance_ - amount_;
    }
}
