"""Offline regression tests for the release and deployment inspection boundaries."""

import base64
import importlib.util
from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("ledger_mainnet", Path(__file__).with_name("mainnet.py"))
ledger = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ledger)


class MainnetTests(unittest.TestCase):
    def setUp(self):
        self.authority = ledger.base58(bytes([2]) * 32)
        self.programdata = bytes([3]) * 32
        self.code = b"\x7fELFfixture"
        self.program = self.account((2).to_bytes(4, "little") + self.programdata, True)
        self.data_bytes = ((3).to_bytes(4, "little") + (123).to_bytes(8, "little")
                           + b"\1" + bytes([2]) * 32 + self.code)

    @staticmethod
    def account(data, executable=False):
        return {"owner": ledger.LOADER, "executable": executable,
                "data": [base64.b64encode(data).decode(), "base64"]}

    def test_public_key_roundtrip_and_invalid_inputs(self):
        for address in (ledger.SIMULATOR_ID, ledger.LOADER, ledger.SYSTEM, self.authority):
            self.assertEqual(ledger.base58(ledger.public_key(address)), address)
        for address in ("", "0" * 44, "1" * 31, "1" * 33, "z" * 44):
            with self.assertRaises(ValueError):
                ledger.public_key(address)

    def test_simulator_and_mismatched_program_ids_are_rejected(self):
        with self.assertRaisesRegex(ValueError, "production program ID"):
            ledger.production_id(ledger.SIMULATOR_ID)
        with patch.object(ledger, "source_id", return_value=ledger.SIMULATOR_ID):
            with self.assertRaisesRegex(ValueError, "differs"):
                ledger.production_id(self.authority)

    def test_mainnet_identity_is_mandatory(self):
        ledger.mainnet(lambda method: ledger.MAINNET)
        for network in ("devnet", "", None):
            with self.assertRaisesRegex(ValueError, "not Solana mainnet"):
                ledger.mainnet(lambda method: network)

    def test_rent_includes_headers_and_temporary_buffer(self):
        sizes = []

        def rpc(method, params):
            self.assertEqual(method, "getMinimumBalanceForRentExemption")
            sizes.append(params[0])
            return params[0] * 10

        quote = ledger.rent_quote(rpc, 1000)
        self.assertEqual(sizes, [36, 1045, 1037])
        self.assertEqual(quote["retained_lamports"], 10810)
        self.assertEqual(quote["conservative_peak_rent_lamports"], 21180)
        with self.assertRaises(ValueError):
            ledger.rent_quote(lambda method, params: None, 1000)

    def test_exact_binary_authority_and_zero_padding_are_accepted(self):
        result = ledger.verify_program(self.program, self.account(self.data_bytes + bytes(32)),
                                       self.code, self.authority)
        self.assertEqual(result["deployment_slot"], 123)
        self.assertEqual(result["programdata"], ledger.base58(self.programdata))

    def test_wrong_authority_immutable_and_modified_code_are_rejected(self):
        bad_data = [self.data_bytes[:12] + b"\0" + self.data_bytes[13:],
                    self.data_bytes[:13] + bytes([4]) * 32 + self.data_bytes[45:],
                    self.data_bytes[:-1] + b"x", self.data_bytes + b"\1", self.data_bytes[:40]]
        for data in bad_data:
            with self.assertRaises(ValueError):
                ledger.verify_program(self.program, self.account(data), self.code, self.authority)

    def test_wrong_loader_and_nonexecutable_program_are_rejected(self):
        for program in (None, {**self.program, "owner": ledger.SYSTEM},
                        {**self.program, "executable": False}):
            with self.assertRaises(ValueError):
                ledger.verify_program(program, self.account(self.data_bytes), self.code, self.authority)

    def test_release_hash_and_commit_are_bound_to_reviewed_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            manifest = {"schema_version": 1, "gate": "scripts/check.sh passed", "program_id": self.authority,
                        "commit": "a" * 40, "sha256": ledger.sha256(self.code), "bytes": len(self.code)}
            (directory / "release.json").write_text(json.dumps(manifest))
            (directory / "cavalre_ledgers_solana.so").write_bytes(self.code)
            with patch.object(ledger, "source_id", return_value=self.authority), \
                    patch.object(ledger, "git", side_effect=lambda *args: "a" * 40 if args[0] == "rev-parse" else ""):
                ledger.load_release(directory)
                (directory / "cavalre_ledgers_solana.so").write_bytes(self.code + b"x")
                with self.assertRaisesRegex(ValueError, "size or hash"):
                    ledger.load_release(directory)
            with patch.object(ledger, "source_id", return_value=self.authority), \
                    patch.object(ledger, "git", return_value="b" * 40):
                with self.assertRaisesRegex(ValueError, "commit"):
                    ledger.load_release(directory)

    def test_preflight_rejects_underfunding_and_occupied_program_address(self):
        program_id = ledger.base58(bytes([7]) * 32)
        payer_id = ledger.base58(bytes([8]) * 32)
        required = (36 + 45 + len(self.code) + 37 + len(self.code)) * 10 + 1000
        args = SimpleNamespace(release_dir=Path("unused"), payer=payer_id,
                               upgrade_authority=self.authority, fee_budget_lamports=1000)
        manifest = {"commit": "a" * 40, "program_id": program_id, "sha256": ledger.sha256(self.code)}
        payer = {"owner": ledger.SYSTEM, "executable": False, "data": ["", "base64"], "lamports": required}
        occupied = False

        def rpc(method, params):
            if method == "getMinimumBalanceForRentExemption":
                return params[0] * 10
            self.assertEqual(method, "getAccountInfo")
            return {"value": payer if params[0] == payer_id or occupied else None}

        with patch.object(ledger, "load_release", return_value=(manifest, self.code)), redirect_stdout(io.StringIO()):
            ledger.preflight(args, rpc)
            payer["lamports"] -= 1
            with self.assertRaisesRegex(ValueError, "does not cover"):
                ledger.preflight(args, rpc)
            occupied = True
            with self.assertRaisesRegex(ValueError, "already exists"):
                ledger.preflight(args, rpc)


if __name__ == "__main__":
    unittest.main()
