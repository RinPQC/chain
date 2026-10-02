#!/usr/bin/env python3
"""Destructive acceptance run, restricted to an explicitly named test Compose project."""
import json
import os
import subprocess
import time

project = os.environ.get("COMPOSE_PROJECT_NAME", "")
if not project.startswith("rinpqc-test-"):
    raise SystemExit("Set a fresh COMPOSE_PROJECT_NAME beginning with rinpqc-test-; this test deletes its volumes")


def run(*args, timeout=60):
    stream = args[0] in ("fresh", "up")
    result = subprocess.run(["bash", "scripts/devnet.sh", *args], capture_output=not stream, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"{args}: {result.stdout}\n{result.stderr}")
    return result.stdout or ""


def rpc(node, method, *args):
    return json.loads(run("rpc", node, method, *args))["result"]


def wait(description, condition, seconds=150):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            result = condition()
            if result:
                print(f"PASS {description}", flush=True)
                return result
        except (RuntimeError, ValueError, KeyError) as error:
            last = error
        time.sleep(2)
    raise RuntimeError(f"Timed out: {description}: {last}")


nodes = [f"node{i}" for i in range(4)]
for index in range(4):
    exists = subprocess.run(["docker", "volume", "inspect", f"{project}_node{index}"], capture_output=True)
    if exists.returncode == 0:
        raise SystemExit("Test project already has state; choose a fresh project name")
try:
    print(run("fresh", timeout=1800), flush=True)
    absent = "node" + run("first-proposer").strip()
    active = [node for node in nodes if node != absent]
    wait("three validators progress with initial proposer absent", lambda: all(int(rpc(node, "metrics")["committed_height"]) >= 4 for node in active))
    run("start", absent)
    wait("unused validator verifies missed history", lambda: int(rpc(absent, "metrics")["sync_verified_total"]) > 0)

    def converged():
        states = [rpc(node, "chain-status") for node in nodes]
        return states[0] if all(state == states[0] for state in states) else None
    initial = wait("four matching verified heads", converged)
    payment = json.loads(run("pay", "30"))
    tx_id = payment["tx_id"]
    wait("payment finalized by every node", lambda: all(rpc(node, "transaction", tx_id)["status"] == "finalized" for node in nodes))
    assert json.loads(run("retry", payment["signed_file"]))["result"]["status"] == "finalized"
    assert rpc("node0", "account", payment["recipient"])["balance"] == "30"
    assert rpc("node0", "account", payment["sender"])["balance"] == "3999970"
    before = int(rpc("node0", "metrics")["committed_height"])
    run("restart", "node0")
    wait("restart retains verified history", lambda: int(rpc("node0", "metrics")["recovered_height"]) >= before)
    assert rpc("node0", "transaction", tx_id)["status"] == "finalized"

    run("partition", "node0", "node1")
    time.sleep(8)  # Allow already certified/in-flight decisions to settle.
    heights = [rpc(node, "metrics")["committed_height"] for node in nodes]
    time.sleep(8)
    assert heights == [rpc(node, "metrics")["committed_height"] for node in nodes], "finalized without a connected quorum"
    print("PASS partition prevents new finality", flush=True)
    run("reconnect", "node0", "node1")
    wait("reconnection restores finality", lambda: all(int(rpc(node, "metrics")["committed_height"]) > int(height) for node, height in zip(nodes, heights)))
    run("down")
    run("up", timeout=1800)
    wait("ordinary down/up retains chain and receipts", lambda: rpc("node0", "chain-status")["chain_id"] == initial["chain_id"] and rpc("node0", "transaction", tx_id)["status"] == "finalized")
    assert json.loads(run("retry", payment["signed_file"]))["result"]["status"] == "finalized"
    assert rpc("node0", "account", payment["recipient"])["balance"] == "30"
    assert rpc("node0", "account", payment["sender"])["balance"] == "3999970"
    wait("matching heads after normal restart", converged)
    print(run("status"), flush=True)
finally:
    print(run("logs"), flush=True)
    print(run("reset", "--discard-test-state"), flush=True)
    for index in range(4):
        result = subprocess.run(["docker", "volume", "inspect", f"{project}_node{index}"], capture_output=True)
        assert result.returncode != 0, "explicit reset left a test volume behind"
