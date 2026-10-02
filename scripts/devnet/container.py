#!/usr/bin/env python3
"""Disposable four-validator setup and local container operations. No external packages."""
import ipaddress
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

BINARY = os.environ.get("RINPQC_NODE_BIN", "/usr/local/bin/rinpqc-node")
ROOT = Path(os.environ.get("DEVNET_ROOT", "/devnet"))
STATE = Path(os.environ.get("DEVNET_STATE", "/state"))
LABEL = "RinPQC disposable M1 devnet v1"


def run(*args):
    result = subprocess.run([BINARY, *map(str, args)], capture_output=True, text=True, timeout=15)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or "Node command failed")
    return result.stdout.strip()


def write(path, value):
    with path.open("x", encoding="utf-8") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())


def metadata(path=STATE):
    data = json.loads((path / "devnet.json").read_text())
    if data.get("label") != LABEL:
        raise RuntimeError("Unrecognized devnet metadata; refusing to reuse data")
    return data


def initialize():
    prefix = os.environ.get("DEVNET_PREFIX", "172.30.91")
    ipaddress.IPv4Network(prefix + ".0/24")
    directories = [ROOT / f"node{i}" for i in range(4)]
    if all((directory / "initialized").is_file() for directory in directories):
        expected = metadata(directories[0])
        for directory in directories:
            if metadata(directory) != expected or expected["prefix"] != prefix:
                raise RuntimeError("Existing state uses different devnet configuration")
            run("config-check", directory / "node.toml")
        print(json.dumps({"event": "DEVNET_REUSED", "chain_id": expected["chain_id"]}))
        return
    if any(list(directory.iterdir()) for directory in directories):
        raise RuntimeError("Partial or foreign state: inspect it or explicitly reset this disposable devnet")
    validators, networks = [], []
    for directory in directories:
        validators.append(run("keygen", "validator", directory / "validator.key"))
        networks.append(run("keygen", "network", directory / "network.key"))
    funded = run("keygen", "transaction", directories[0] / "funded.key")
    run("genesis-create", directories[0] / "genesis.json", *validators, funded)
    genesis = (directories[0] / "genesis.json").read_text()
    chain = dict(line.split("=", 1) for line in run("genesis-check", directories[0] / "genesis.json").splitlines())["chain_id"]
    data = {"label": LABEL, "prefix": prefix, "chain_id": chain, "funded_account": funded,
            "validators": validators, "first_proposer": validators.index(min(validators))}
    for index, directory in enumerate(directories):
        if index:
            write(directory / "genesis.json", genesis)
        text = (f'version = 1\ngenesis = "genesis.json"\nexpected_chain_id = "{chain}"\n'
                f'validator_key = "validator.key"\nnetwork_key = "network.key"\ndata_dir = "chain"\n'
                f'listen = "0.0.0.0:{31001 + index}"\nrpc_listen = "127.0.0.1:{32001 + index}"\n')
        for peer in range(4):
            if peer != index:
                text += f'\n[[peers]]\naddress = "{prefix}.{10 + peer}:{31001 + peer}"\npublic_key = "{networks[peer]}"\n'
        write(directory / "node.toml", text)
        write(directory / "devnet.json", json.dumps(data))
        run("init", directory / "node.toml")
    for directory in directories:
        write(directory / "initialized", LABEL + "\n")
    print(json.dumps({"event": "DEVNET_INITIALIZED", "chain_id": chain, "first_proposer": data["first_proposer"]}))


def rpc(command, *args):
    data = metadata()
    index = int(os.environ["NODE_INDEX"])
    return json.loads(run(command, f"127.0.0.1:{32001 + index}", data["chain_id"], *args))


def pay(amount):
    if os.environ["NODE_INDEX"] != "0":
        raise RuntimeError("The disposable funded key is available only on node0")
    data = metadata()
    directory = STATE / "payments"
    directory.mkdir(mode=0o700, exist_ok=True)
    name = uuid.uuid4().hex
    recipient = run("keygen", "transaction", directory / (name + ".key"))
    nonce = rpc("account", data["funded_account"])["result"]["next_nonce"]
    signed = directory / (name + ".bin")
    result = json.loads(run("payment-sign", STATE / "funded.key", STATE / "genesis.json", recipient, amount, nonce, signed))
    submission = json.loads(run("payment-submit", "127.0.0.1:32001", signed))
    print(json.dumps({**result, "sender": data["funded_account"], "recipient": recipient, "signed_file": signed.name, "submission": submission}))


def main(args):
    os.umask(0o077)
    if args == ["initialize"]:
        initialize()
    elif args == ["first-proposer"]:
        print(metadata(ROOT / "node0")["first_proposer"])
    elif args == ["assert-unused"]:
        if any((ROOT / f"node{i}" / "ever_started").exists() for i in range(4)):
            raise RuntimeError("Fresh-join scenario requires unused identities; use a new project or explicit reset")
    elif args == ["node"]:
        metadata()
        if not (STATE / "initialized").is_file():
            raise RuntimeError("Devnet initialization is incomplete")
        if not (STATE / "ever_started").exists():
            write(STATE / "ever_started", LABEL + "\n")
        os.execv(BINARY, [BINARY, "start", str(STATE / "node.toml")])
    elif args == ["health"]:
        rpc("chain-status")
    elif args and args[0] == "rpc":
        if len(args) < 2 or args[1] not in ("chain-status", "metrics", "account", "transaction"):
            raise RuntimeError("Use rpc chain-status|metrics|account|transaction [public ID]")
        print(json.dumps(rpc(args[1], *args[2:])))
    elif len(args) == 2 and args[0] == "pay":
        pay(args[1])
    elif len(args) == 2 and args[0] == "retry":
        if Path(args[1]).name != args[1] or not args[1].endswith(".bin"):
            raise RuntimeError("Use the signed_file basename returned by pay")
        print(run("payment-submit", "127.0.0.1:32001", STATE / "payments" / args[1]))
    else:
        raise RuntimeError("Unknown devnet container command")


if __name__ == "__main__":
    try:
        main(sys.argv[1:])
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"Devnet error: {error}", file=sys.stderr)
        sys.exit(1)
