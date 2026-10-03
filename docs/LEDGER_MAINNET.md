# Ledger mainnet deployment

This runbook deploys only `crates/cavalre-ledgers-solana`, the independent omnibus service.
It does not deploy SR, Multiswap, demo programs, or the Ledger test consumer.
No mainnet program has been deployed by the preparation work.

## Current readiness

The October 2, 2026 preparation run passed `bash scripts/check.sh`: formatting,
Clippy, fresh sBPF compilation, and all 115 Rust tests, including the 32 standalone
Ledger tests. Nine additional Python tests cover the deployment checks. The
current binary is 408,096 bytes. These are local verification
results, not an independent audit or a mainnet smoke test.

The program still declares the simulation identity
`DSXaqgjqGtTWfvk89dvXgii4x6EimeALFjhbxnN3mYmy`. A production ID must be selected
and committed before creating the release. `mainnet.py prepare` rejects that
simulation identity. No accounting rules, account layouts, or authorities are
changed by this preparation workflow.

## Addresses and signing

The initial operational setup is a **1-of-1 Squads multisig**. The signer device
is still being selected: Eric has a Ledger Flex and has ordered a Solana Seeker.
Members and the approval threshold can change later without changing CavalRe
Ledger's program address.

| Address | Purpose |
| --- | --- |
| Program ID | Permanent address of CavalRe Ledgers. Its keypair signs initial account creation; it does not grant upgrade authority. |
| Deployment wallet | Holds enough SOL to pay rent and upload transactions. Initially controls the empty deployed program, then hands authority to Squads. |
| Squads vault address | Final upgrade authority. The Squad's signer approves program management through Squads. |

Use the **Squads vault address**, not the multisig configuration address or the
member address, as the final upgrade authority. Verify it in Squads before
the handoff. The preparation tool checks the final onchain authority against the
supplied address; it does not inspect Squad membership or approval thresholds.

For the Flex, initialize it yourself using Ledger's official setup flow,
generate its recovery phrase on the device, and keep the phrase offline. Connect it as a
hardware wallet in Phantom or Solflare. Do not import its recovery phrase into
a browser wallet. Squads' current instructions require blind signing for this
connection; review the transaction in Squads before approval.

For the Seeker, use Seed Vault Wallet to retain Seed Vault's hardware-backed
key custody. Solana Mobile documents its Mobile Wallet Adapter support in
Android Chrome. Squads documents mobile access through wallet browsers, but
the specific Squads/Seed Vault Wallet connection must still be tested on the
device. Complete proposal creation, approval and execution before selecting
that address as the sole signer. Importing the seed into an ordinary software
wallet to get a connection would change the key-security model.

The Seeker's delivery is not a deployment dependency. Start with the available
device if necessary and change membership later. If both devices eventually
become separate signers, use independent seeds; a copy of one seed is recovery
of the same signer.

Create the 1-of-1 Squad and rehearse an approval and execution before assigning
the program to its vault. The upgrade authority can replace Ledger's code, so
it is a separate trust boundary from ordinary users' account controllers.

## Select the program identity

Run on the operator's computer, outside the repository. These commands create
a fresh program identity, not a funded wallet or a multisig signer:

```bash
install -d -m 700 "$HOME/.config/cavalre"
solana-keygen new --outfile "$HOME/.config/cavalre/ledger-mainnet-program-keypair.json"
solana-keygen pubkey "$HOME/.config/cavalre/ledger-mainnet-program-keypair.json"
```

Share only the public address. Replace `declare_id!` in
`crates/cavalre-ledgers-solana/src/lib.rs` with that address, update its simulation-only
comment, and commit the change. Tests and CPI callers use the crate's declared
ID. Keep the keypair outside Git and do not reuse a build-generated keypair.

Prepare a separately funded deployment wallet. Its signing material also stays
on the operator's computer. The selected device remains the Squad signer;
it need not sign each program-upload transaction.

## Build the release

Use the repository's pinned Rust, Agave 4.3.0 and platform-tools v1.57. Start from
the clean, reviewed production-ID commit. From the repository root:

```bash
export PATH="$PWD/target/toolchains/solana-release/bin:$PATH"
export LEDGER_PROGRAM_ID="$(solana-keygen pubkey "$HOME/.config/cavalre/ledger-mainnet-program-keypair.json")"
python3 -B scripts/ledgers/test_mainnet.py
python3 scripts/ledgers/mainnet.py prepare --program-id "$LEDGER_PROGRAM_ID"
```

`prepare` runs the full repository gate and copies only `cavalre_ledgers_solana.so` into
`target/ledgers-release`, alongside a manifest binding its SHA-256, size, program
ID, source commit, Cargo lockfile and toolchain versions. It refuses a dirty
checkout, a mismatched ID, or an existing release directory. Keep this exact
checkout and release for subsequent checks.

This is a pinned local release with onchain byte comparison. It is not a claim
of a container-reproduced or publicly registered verified build. The repository
remains private; no verification service is given source access by this workflow.

## Read-only mainnet preflight

Set `LEDGER_RPC_URL` to a mainnet provider endpoint, or omit it to use the public
Solana endpoint. Provider credentials are not written to reports. Set
`LEDGER_SQUADS_VAULT` to the reviewed vault public address and
`LEDGER_DEPLOYER_KEYPAIR` to the external deployment wallet file.

```bash
export LEDGER_PAYER="$(solana-keygen pubkey "$LEDGER_DEPLOYER_KEYPAIR")"
python3 scripts/ledgers/mainnet.py preflight \
  --payer "$LEDGER_PAYER" \
  --upgrade-authority "$LEDGER_SQUADS_VAULT" \
  --fee-budget-lamports 50000000
```

The 0.05 SOL allowance above is an operator-selected transaction-fee budget,
not an RPC fee estimate or an enforced spending cap. Adjust it after reviewing
network conditions. The tool checks mainnet's genesis hash, an unused program
address, the release, the payer, and live rent including a temporary upload
buffer. It sends no transactions and reads no signing keys.

For size and live rent inspection before a production identity is selected:

```bash
python3 scripts/ledgers/mainnet.py inspect
```

The October 2 quote for the current 408,096-byte build was **2.074839640 SOL**
retained in the Program and ProgramData accounts and **4.148805520 SOL** for a
conservative peak including the temporary buffer, before fees. The buffer rent
is reclaimed after a successful deployment. Rerun the quote for the final
binary. Squads setup and subsequent Ledger account creation have separate costs.

## Deployment and handoff

These are transaction-sending steps for the operator to execute after reviewing
the release, preflight report, keys, and destination authority. Preparation does
not run them. First compare the local key's public address with the release:

```bash
export LEDGER_PROGRAM_ID="$(solana-keygen pubkey "$HOME/.config/cavalre/ledger-mainnet-program-keypair.json")"
python3 - "$LEDGER_PROGRAM_ID" <<'PY'
import json, sys
with open("target/ledgers-release/release.json") as file:
    release = json.load(file)
if sys.argv[1] != release["program_id"]:
    raise SystemExit("Program keypair does not match the release")
PY
cat target/ledgers-release/release.json
```

Deploy with explicit payer and authority, using exactly the reviewed allocation.
Keep the CLI's feature verification and transaction preflight enabled. Do not
use `--final`, which permanently removes upgrade authority.

```bash
solana --url "${LEDGER_RPC_URL:-https://api.mainnet-beta.solana.com}" \
  --keypair "$LEDGER_DEPLOYER_KEYPAIR" program deploy \
  target/ledgers-release/cavalre_ledgers_solana.so \
  --program-id "$HOME/.config/cavalre/ledger-mainnet-program-keypair.json" \
  --upgrade-authority "$LEDGER_DEPLOYER_KEYPAIR" \
  --fee-payer "$LEDGER_DEPLOYER_KEYPAIR" \
  --max-len "$(python3 -c 'import json; print(json.load(open("target/ledgers-release/release.json"))["bytes"])')" \
  --use-rpc
python3 scripts/ledgers/mainnet.py verify --upgrade-authority "$LEDGER_PAYER"
```

After the deployment is finalized and the binary matches, register the program
in Squads and transfer authority to its reviewed vault. No Ledger custody funds
are needed for this step. A Squads vault is a PDA, so it cannot provide an
ordinary keypair signature for the CLI handoff; that is why this specific
signer-check flag is necessary. Confirm the vault address independently first.

```bash
solana --url "${LEDGER_RPC_URL:-https://api.mainnet-beta.solana.com}" \
  --keypair "$LEDGER_DEPLOYER_KEYPAIR" program set-upgrade-authority \
  "$LEDGER_PROGRAM_ID" \
  --upgrade-authority "$LEDGER_DEPLOYER_KEYPAIR" \
  --new-upgrade-authority "$LEDGER_SQUADS_VAULT" \
  --skip-new-upgrade-authority-signer-check
python3 scripts/ledgers/mainnet.py verify --upgrade-authority "$LEDGER_SQUADS_VAULT"
```

`verify` checks finalized loader ownership, executable state, the expected
upgrade authority, every binary byte and any zero padding. Save its public
receipt with the release. If deployment times out, inspect program and buffer
state before retrying; do not create another program identity or close buffers
indiscriminately.

## Acceptance before application funding

Complete a small live round trip under a dedicated namespace: register a
supported mint, open two controlled positions, deposit, transfer claims, and
withdraw. Confirm exact native balances, total claims and controller behavior.
Rehearse a Squads-controlled upgrade on devnet before relying on the recovery
path. These live exercises and the final-ID release remain outstanding; the
local suite does not substitute for them. An independent audit also remains
outstanding.

Once accepted, record the program ID, ProgramData address, final authority,
source commit, binary hash and transaction signatures. Publishing a program
does not initialize a global CavalRe-owned namespace: applications create their
own namespaces and retain their own spending controls.

## References

- [Solana program deployment](https://solana.com/docs/programs/deploying)
- [Solana verified builds](https://solana.com/docs/programs/verified-builds)
- [Create a Squad and connect a Ledger device](https://docs.squads.so/main/getting-started/create-a-squad)
- [Squads program management](https://docs.squads.so/main/navigating-your-squad/developers-assets/programs)
- [Squads vault as upgrade authority](https://docs.layerzero.network/v2/developers/solana/technical-reference/solana-guidance)
- [Seeker Seed Vault](https://docs.solanamobile.com/solana-mobile-stack/seed-vault)
- [Seed Vault Wallet and Android Chrome](https://docs.solanamobile.com/get-started/web/apps)
- [Squads on mobile](https://docs.squads.so/main/additional-resources/squads-on-mobile)
