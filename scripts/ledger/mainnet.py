#!/usr/bin/env python3
"""Prepare Ledger releases and inspect mainnet. Never signs or sends transactions."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from datetime import datetime, timezone
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "crates/ledger-solana/src/lib.rs"
BINARY = ROOT / "target/deploy/cavalre_ledger_solana.so"
RELEASE = ROOT / "target/ledger-release"
SIMULATOR_ID = "DSXaqgjqGtTWfvk89dvXgii4x6EimeALFjhbxnN3mYmy"
MAINNET = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d"
LOADER = "BPFLoaderUpgradeab1e11111111111111111111111"
SYSTEM = "11111111111111111111111111111111"
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def public_key(value):
    require(isinstance(value, str) and 32 <= len(value) <= 44, "Invalid public key")
    number = 0
    for char in value:
        require(char in BASE58, "Invalid public key")
        number = number * 58 + BASE58.index(char)
    decoded = bytes(len(value) - len(value.lstrip("1"))) + number.to_bytes(
        (number.bit_length() + 7) // 8, "big"
    )
    require(len(decoded) == 32, "Public key must decode to 32 bytes")
    return decoded


def base58(data):
    number, text = int.from_bytes(data, "big"), ""
    while number:
        number, remainder = divmod(number, 58)
        text = BASE58[remainder] + text
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + text


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()


def source_id():
    ids = re.findall(r'declare_id!\("([^"]+)"\)', SOURCE.read_text())
    require(len(ids) == 1, "Expected one Ledger program ID")
    public_key(ids[0])
    return ids[0]


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def now():
    return datetime.now(timezone.utc).isoformat()


def production_id(program_id):
    public_key(program_id)
    require(program_id not in (SIMULATOR_ID, SYSTEM, LOADER), "Select a production program ID first")
    require(program_id == source_id(), "Program ID differs from Ledger declare_id!")


def binary(path):
    code = path.read_bytes()
    require(code[:4] == b"\x7fELF", "Ledger artifact is not an ELF executable")
    return code


class Rpc:
    def __init__(self, url):
        require(url.startswith("https://"), "Mainnet RPC must use HTTPS")
        self.url = url

    def __call__(self, method, params=None):
        request = urllib.request.Request(
            self.url,
            json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode(),
            {"Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                result = json.load(response)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            # Provider URLs can contain credentials. Do not echo them in errors or reports.
            raise ValueError(f"RPC {method} failed ({type(error).__name__})") from None
        require(result.get("id") == 1 and "result" in result and "error" not in result,
                f"RPC {method} returned an error or invalid response")
        return result["result"]


def mainnet(rpc):
    require(rpc("getGenesisHash") == MAINNET, "RPC is not Solana mainnet-beta")


def lamports(value):
    require(type(value) is int and value >= 0, "Invalid lamport amount from RPC")
    return value


def rent_quote(rpc, code_size):
    # Loader-v3 layouts: Program=36, ProgramData header=45, Buffer header=37.
    # Allocate exactly the binary size, matching the documented --max-len command.
    program = lamports(rpc("getMinimumBalanceForRentExemption", [36]))
    data = lamports(rpc("getMinimumBalanceForRentExemption", [45 + code_size]))
    buffer = lamports(rpc("getMinimumBalanceForRentExemption", [37 + code_size]))
    return {"program_lamports": program, "programdata_lamports": data,
            "retained_lamports": program + data, "buffer_lamports": buffer,
            "conservative_peak_rent_lamports": program + data + buffer}


def account(rpc, address, min_slot=None):
    config = {"encoding": "base64", "commitment": "finalized"}
    if min_slot is not None:
        config["minContextSlot"] = min_slot
    return rpc("getAccountInfo", [address, config])


def account_data(info):
    require(isinstance(info.get("data"), list) and len(info["data"]) == 2
            and info["data"][1] == "base64", "Unexpected account encoding")
    return base64.b64decode(info["data"][0], validate=True)


def verify_program(program, data, code, authority):
    require(program is not None and program["owner"] == LOADER and program["executable"],
            "Program is absent or is not an executable loader-v3 program")
    p = account_data(program)
    require(len(p) == 36 and int.from_bytes(p[:4], "little") == 2, "Invalid Program account")
    require(data is not None and data["owner"] == LOADER and not data["executable"],
            "Invalid ProgramData owner or executable flag")
    d = account_data(data)
    require(len(d) >= 45 + len(code) and int.from_bytes(d[:4], "little") == 3,
            "Invalid ProgramData layout")
    require(d[12] == 1 and d[13:45] == public_key(authority), "Upgrade authority does not match")
    require(d[45:45 + len(code)] == code and not any(d[45 + len(code):]),
            "Onchain binary differs from the release")
    return {"programdata": base58(p[4:36]), "deployment_slot": int.from_bytes(d[4:12], "little"),
            "upgrade_authority": authority, "sha256": sha256(code)}


def load_release(directory):
    manifest = json.loads((directory / "release.json").read_text())
    require(manifest.get("schema_version") == 1 and manifest.get("gate") == "scripts/check.sh passed",
            "Release has no completed verification gate")
    production_id(manifest["program_id"])
    require(git("rev-parse", "HEAD") == manifest["commit"], "Checkout differs from release commit")
    require(not git("status", "--porcelain", "--untracked-files=normal"), "Checkout must be clean")
    code = binary(directory / "cavalre_ledger_solana.so")
    require(len(code) == manifest["bytes"] and sha256(code) == manifest["sha256"],
            "Release binary size or hash differs from manifest")
    return manifest, code


def prepare(args):
    production_id(args.program_id)
    require(not git("status", "--porcelain", "--untracked-files=normal"), "Commit the reviewed source first")
    require(not args.release_dir.exists(), "Release directory already exists; choose a new --release-dir")
    commit = git("rev-parse", "HEAD")
    env = os.environ.copy()
    local_tools = ROOT / "target/toolchains/solana-release/bin"
    if local_tools.is_dir():
        env["PATH"] = str(local_tools) + os.pathsep + env["PATH"]
    for key in list(env):
        if key.endswith("_PROGRAM_SO") or key in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"):
            del env[key]
    env["CAVALRE_LEDGER_PROGRAM_SO"] = str(BINARY)
    versions = {tool: subprocess.check_output([tool, "--version"], env=env, cwd=ROOT, text=True).strip()
                for tool in ("solana", "cargo-build-sbf", "rustc")}
    require(versions["solana"].startswith("solana-cli 4.3.0 "), "Expected Solana CLI 4.3.0")
    subprocess.run(["bash", "scripts/check.sh"], cwd=ROOT, env=env, check=True)
    require(git("rev-parse", "HEAD") == commit and not git("status", "--porcelain", "--untracked-files=normal"),
            "Source changed during verification")
    code = binary(BINARY)
    require(public_key(args.program_id) in code, "Compiled artifact does not contain the selected program ID")
    manifest = {"schema_version": 1, "commit": commit, "program_id": args.program_id,
                "bytes": len(code), "sha256": sha256(code), "built_at": now(),
                "toolchain": versions, "gate": "scripts/check.sh passed",
                "cargo_lock_sha256": sha256((ROOT / "Cargo.lock").read_bytes())}
    args.release_dir.mkdir(parents=True)
    shutil.copyfile(BINARY, args.release_dir / "cavalre_ledger_solana.so")
    (args.release_dir / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def inspect(args, rpc):
    code = binary(BINARY)
    return {"checked_at": now(), "program_id": source_id(), "simulation_identity": source_id() == SIMULATOR_ID,
            "bytes": len(code), "sha256": sha256(code), "rent": rent_quote(rpc, len(code)),
            "status": "size and rent inspection only; not a production release"}


def preflight(args, rpc):
    manifest, code = load_release(args.release_dir)
    public_key(args.payer)
    public_key(args.upgrade_authority)
    require(args.upgrade_authority not in (SYSTEM, LOADER, SIMULATOR_ID, manifest["program_id"]),
            "Select a usable upgrade-authority address")
    require(args.fee_budget_lamports > 0, "Supply a positive transaction-fee allowance")
    require(account(rpc, manifest["program_id"])["value"] is None,
            "Program address already exists; this workflow is for first deployment only")
    payer = account(rpc, args.payer)["value"]
    require(payer is not None and payer["owner"] == SYSTEM and not payer["executable"]
            and len(account_data(payer)) == 0, "Payer must be a funded ordinary system account")
    rent = rent_quote(rpc, len(code))
    required = rent["conservative_peak_rent_lamports"] + args.fee_budget_lamports
    balance = lamports(payer["lamports"])
    report = {"checked_at": now(), "cluster": "mainnet-beta", "commit": manifest["commit"],
              "program_id": manifest["program_id"], "sha256": manifest["sha256"], "bytes": len(code),
              "payer": args.payer, "intended_final_upgrade_authority": args.upgrade_authority,
              "rent": rent, "fee_allowance_lamports": args.fee_budget_lamports,
              "required_lamports": required, "balance_lamports": balance,
              "funding_sufficient": balance >= required,
              "scope": "read-only checks; signer possession and multisig configuration require separate review"}
    print(json.dumps(report, indent=2))
    require(balance >= required, "Payer does not cover conservative peak rent plus the selected fee allowance")
    return None


def verify(args, rpc):
    manifest, code = load_release(args.release_dir)
    public_key(args.upgrade_authority)
    program_reply = account(rpc, manifest["program_id"])
    program = program_reply["value"]
    require(program is not None and program["owner"] == LOADER and program["executable"], "Program is not deployed")
    raw = account_data(program)
    require(len(raw) == 36 and int.from_bytes(raw[:4], "little") == 2, "Invalid Program layout")
    data_reply = account(rpc, base58(raw[4:36]), program_reply["context"]["slot"])
    result = verify_program(program, data_reply["value"], code, args.upgrade_authority)
    return {"checked_at": now(), "cluster": "mainnet-beta", "program_id": manifest["program_id"],
            "commit": manifest["commit"], "observed_slot": data_reply["context"]["slot"], **result}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("inspect", "prepare", "preflight", "verify"):
        command = sub.add_parser(name)
        if name != "inspect":
            command.add_argument("--release-dir", type=Path, default=RELEASE)
        if name == "prepare":
            command.add_argument("--program-id", required=True)
        if name in ("preflight", "verify"):
            command.add_argument("--upgrade-authority", required=True)
        if name == "preflight":
            command.add_argument("--payer", required=True)
            command.add_argument("--fee-budget-lamports", type=int, required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        result = prepare(args)
    else:
        rpc = Rpc(os.environ.get("LEDGER_RPC_URL", "https://api.mainnet-beta.solana.com"))
        mainnet(rpc)
        result = {"inspect": inspect, "preflight": preflight, "verify": verify}[args.command](args, rpc)
    if result is not None:
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        print(f"Ledger preparation failed: {error}", file=sys.stderr)
        sys.exit(1)
