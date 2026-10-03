"""Verify the exact Git blob identities used to compile the reference fixtures."""
import hashlib
import json
from pathlib import Path
import sys

root = Path(__file__).resolve().parent.parent
contracts = Path(sys.argv[1]).resolve()
manifest = json.loads((root / "spec/fixtures/reference-files.json").read_text())
pin = json.loads((root / "spec/upstream.json").read_text())["references"][0]["commit"]
if manifest["commit"] != pin:
    raise SystemExit("Source pin and fixture manifest disagree; regenerate deliberately.")
for entry in manifest["files"]:
    path = contracts / entry["path"]
    try:
        data = path.read_bytes()
    except FileNotFoundError:
        raise SystemExit(f"Missing {path}; initialize the cavalre-contracts submodule.") from None
    blob = b"blob " + str(len(data)).encode() + b"\0" + data
    if hashlib.sha1(blob).hexdigest() != entry["sha"]:
        raise SystemExit(f"Reference differs from pinned source: {path}")
print(f"Verified {len(manifest['files'])} source/configuration blobs at {pin}")
