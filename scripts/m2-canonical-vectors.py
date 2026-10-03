"""Reproduce structural vectors with independent Python SHA-384 and integer encoding.

Signatures are intentionally invalid all-zero placeholders; no authorization claim.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def integer(size, value):
    return value.to_bytes(size, "big")


def commitment(domain, payload):
    domain = domain.encode("ascii")
    framing = (b"RINPQC/M2/HASH" + integer(2, 2) + integer(2, 1)
               + integer(2, len(domain)) + domain + integer(8, len(payload)))
    return hashlib.sha384(framing + payload).digest()


def prefix(kind):
    return b"RIN2" + integer(2, 2) + integer(2, 1) + integer(1, kind)


def validator_id(key):
    return commitment("VALIDATOR", integer(2, 87) + key)


source = json.loads((ROOT / "tests/fixtures/pq/nist-ml-dsa-87.json").read_text())
keys = [bytes.fromhex(test["pk"]) for group in source["keyGen"] for test in group["tests"]]
keys.append(next(bytes.fromhex(test["pk"]) for group in source["sigVer"]
                 for test in group["tests"] if test["testPassed"]))
keys.sort(key=validator_id)
public_key = keys[0]
account = commitment("ACCOUNT", integer(2, 87) + public_key)
genesis = (prefix(1) + bytes(range(32)) + integer(4, 1048576) + integer(4, 4096)
           + integer(8, 5000) + b"".join(keys) + integer(4, 1) + account + integer(8, 100))
chain = commitment("GENESIS", genesis)
state = commitment("STATE", integer(4, 1) + account + integer(8, 100) + integer(8, 0))
transfer = prefix(2) + chain + account + bytes([9]) * 48 + integer(8, 1) + integer(8, 0)
envelope = prefix(3) + transfer + public_key + bytes(4627)
body = integer(4, 1) + envelope
transactions_root = commitment("TXLIST", body)
header = prefix(4) + chain + integer(8, 1) + chain + transactions_root + state
block = header + body
block_id = commitment("BLOCK", header)
certificate = (prefix(5) + chain + integer(8, 1) + integer(4, 0) + block_id + integer(1, 3)
               + b"".join(validator_id(key) + bytes(4627) for key in keys[:3]))
values = {
    "public_key": public_key, "account_id": account, "validator_id": validator_id(public_key),
    "genesis": genesis, "chain_id": chain, "state_root": state, "transfer": transfer,
    "transaction_id": commitment("TXID", transfer), "envelope": envelope,
    "transactions_root": transactions_root, "header": header, "block_id": block_id,
    "block": block, "certificate": certificate,
}
out = ROOT / "tests/fixtures/m2"
out.mkdir(exist_ok=True)
(out / "canonical.json").write_text(json.dumps({key: value.hex() for key, value in values.items()}, indent=2) + "\n")
print({key: len(value) for key, value in values.items()})
