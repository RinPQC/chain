#!/usr/bin/env python3
"""Destructive acceptance run, restricted to an explicitly named test Compose project."""
import json
import os
from pathlib import Path
import platform
import re
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
evidence = Path(os.environ.get("M1_EVIDENCE_DIR", "artifacts/m1"))
evidence.mkdir(parents=True, exist_ok=True)
report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
          "platform": platform.platform(), "cpu_count": os.cpu_count(), "project": project,
          "docker": subprocess.check_output(["docker", "version", "--format", "{{json .}}"], text=True).strip(),
          "compose": subprocess.check_output(["docker", "compose", "version", "--short"], text=True).strip(),
          "scenarios": [], "success": False}
(evidence / "compose.yaml").write_text(subprocess.check_output(["docker", "compose", "config"], text=True))


def snapshot(name):
    states = {node: rpc(node, "chain-status") for node in nodes}
    report["scenarios"].append({"name": name, "heads": states})
    print(json.dumps({"scenario": name, "heads": states}), flush=True)
    return states


def check_trace():
    seen = {}
    for node in nodes:
        logs = subprocess.check_output(["docker", "compose", "logs", "--no-color", node], text=True)
        (evidence / f"{node}.log").write_text(logs)
        count = 0
        for height, block, root in re.findall(r"COMMITTED height=(\d+) block_id=([0-9a-f]{64}) state_root=([0-9a-f]{64})", logs):
            value = (block, root)
            assert seen.setdefault(height, value) == value, f"conflicting finalized trace at {height}"
            count += 1
        assert count, f"No committed evidence from {node}"
    report["matching_trace_heights"] = len(seen)

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
    (evidence / "genesis.json").write_text(subprocess.check_output(["docker", "compose", "exec", "-T", "node0", "cat", "/state/genesis.json"], text=True))
    container = subprocess.check_output(["docker", "compose", "ps", "-q", "node0"], text=True).strip()
    report["image_id"] = subprocess.check_output(["docker", "inspect", "--format", "{{.Image}}", container], text=True).strip()
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

    run("split", "2-2")
    time.sleep(8)  # Allow already certified/in-flight decisions to settle.
    split = snapshot("2-2 settled")
    time.sleep(12)
    assert split == snapshot("2-2 after 12 seconds"), "finalized without a connected quorum"
    report["partition_rules_2_2"] = run("fault-status")
    print("PASS 2-2 partition prevents new finality", flush=True)
    run("heal")
    wait("2-2 reconnection restores finality", lambda: all(int(rpc(node, "chain-status")["height"]) > int(split[node]["height"]) for node in nodes))
    wait("matching heads after 2-2", converged)
    snapshot("2-2 recovered")

    run("split", "3-1")
    time.sleep(8)
    split = snapshot("3-1 settled")
    majority = nodes[:3]
    wait("3-1 majority advances by two blocks", lambda: all(int(rpc(node, "chain-status")["height"]) >= int(split[node]["height"]) + 2 for node in majority))
    assert rpc("node3", "chain-status") == split["node3"], "isolated validator finalized alone"
    snapshot("3-1 majority advanced, minority stopped")
    report["partition_rules_3_1"] = run("fault-status")
    run("heal")
    wait("matching heads after 3-1", converged)
    snapshot("3-1 recovered")
    check_trace()  # Capture all pre-recreation container logs.
    run("down")
    run("up", timeout=1800)
    wait("ordinary down/up retains chain and receipts", lambda: rpc("node0", "chain-status")["chain_id"] == initial["chain_id"] and rpc("node0", "transaction", tx_id)["status"] == "finalized")
    assert json.loads(run("retry", payment["signed_file"]))["result"]["status"] == "finalized"
    assert rpc("node0", "account", payment["recipient"])["balance"] == "30"
    assert rpc("node0", "account", payment["sender"])["balance"] == "3999970"
    wait("matching heads after normal restart", converged)
    snapshot("persistent restart converged")
    report["final_payment_state"] = {}
    for node in nodes:
        state = {"sender": rpc(node, "account", payment["sender"]),
                 "recipient": rpc(node, "account", payment["recipient"]),
                 "receipt": rpc(node, "transaction", tx_id)}
        assert state["sender"]["next_nonce"] == "1"
        assert state["sender"]["balance"] == "3999970"
        assert state["recipient"]["balance"] == "30"
        assert state["receipt"]["status"] == "finalized"
        report["final_payment_state"][node] = state
    report["payment"] = {key: payment[key] for key in ("tx_id", "sender", "recipient")}
    report["success"] = True
    print(run("status"), flush=True)
finally:
    (evidence / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    logs = run("logs")
    (evidence / "final-containers.log").write_text(logs)
    print(logs, flush=True)
    print(run("reset", "--discard-test-state"), flush=True)
    for index in range(4):
        result = subprocess.run(["docker", "volume", "inspect", f"{project}_node{index}"], capture_output=True)
        assert result.returncode != 0, "explicit reset left a test volume behind"
