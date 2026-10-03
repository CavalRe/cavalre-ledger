// SPDX-License-Identifier: MIT
pragma solidity 0.8.26;

interface ILedgerToken {
    function balanceOf(address account_) external view returns (uint256);
    function transfer(address to_, uint256 amount_) external returns (bool);
    function transferFrom(address from_, address to_, uint256 amount_) external returns (bool);
}

/// @dev Accept standard ERC20 returns and tokens that return no data.
library LedgerToken {
    error TokenTransferFailed();

    function safeTransferCall(address token_, bytes memory data_) internal {
        if (token_.code.length == 0) revert TokenTransferFailed();
        (bool success_, bytes memory result_) = token_.call(data_);
        if (!success_ || (result_.length != 0 && (result_.length != 32 || !abi.decode(result_, (bool))))) {
            revert TokenTransferFailed();
        }
    }
}

/// @notice One ERC20 vault per namespace and token. Only Ledgers can release collateral.
contract LedgerVault {
    error Unauthorized();

    address public immutable ledger;
    address public immutable token;

    constructor(address token_) {
        if (token_ == address(0)) revert LedgerToken.TokenTransferFailed();
        ledger = msg.sender;
        token = token_;
    }

    function send(address recipient_, uint128 amount_) external {
        if (msg.sender != ledger) revert Unauthorized();
        LedgerToken.safeTransferCall(token, abi.encodeCall(ILedgerToken.transfer, (recipient_, amount_)));
    }
}
