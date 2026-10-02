#!/usr/bin/env python3
"""Packet-drop partitions inside disposable validator namespaces, never the host namespace."""
import json
import os
import subprocess
import sys

# Executed in a short-lived, mount-free helper sharing only one validator's network.
RULES = r'''
import subprocess, sys
chain = "RINPQC_TEST"
def ip(*args, check=True):
    return subprocess.run(["iptables", "-w", "5", *args], check=check, text=True, capture_output=True)
exists = ip("-S", chain, check=False)
if exists.returncode not in (0, 1):
    raise RuntimeError(exists.stderr)
if sys.argv[1] == "heal":
    if exists.returncode == 0:
        ip("-D", "INPUT", "-j", chain)
        ip("-F", chain)
        ip("-X", chain)
elif sys.argv[1] == "status":
    if exists.returncode == 0:
        print(ip("-L", chain, "-n", "-v", "-x").stdout)
    else:
        print("No test partition rules")
else:
    if exists.returncode == 0:
        raise RuntimeError("Partition already installed; heal it first")
    ip("-N", chain)
    for address in sys.argv[2:]:
        ip("-A", chain, "-s", address, "-j", "DROP")
    ip("-I", "INPUT", "1", "-j", chain)
    print(ip("-S", chain).stdout)
'''


def main(args):
    if args not in (["2-2"], ["3-1"], ["heal"], ["status"]):
        raise SystemExit("Use faults.py 2-2|3-1|heal|status")
    project = os.environ.get("COMPOSE_PROJECT_NAME", "rinpqc-devnet")
    containers = []
    for i in range(4):
        cid = subprocess.check_output(["docker", "compose", "ps", "-q", f"node{i}"], text=True).strip()
        if not cid:
            raise RuntimeError(f"node{i} must be running")
        info = json.loads(subprocess.check_output(["docker", "inspect", cid], text=True))[0]
        labels = info["Config"]["Labels"]
        if (labels.get("com.docker.compose.project") != project
                or labels.get("com.docker.compose.service") != f"node{i}"
                or info["HostConfig"]["NetworkMode"] == "host"):
            raise RuntimeError("Refusing a foreign or host-network container")
        network = info["NetworkSettings"]["Networks"][project + "_consensus"]
        if not network["IPAddress"]:
            raise RuntimeError("Reconnect bridge interfaces before using packet-drop partitions")
        containers.append((cid, info["Image"], network["IPAddress"]))
    groups = [0, 0, 1, 1] if args == ["2-2"] else [0, 0, 0, 1]
    for i, (cid, image, _) in enumerate(containers):
        blocked = [ip for j, (_, _, ip) in enumerate(containers) if groups[j] != groups[i]]
        print(f"node{i}: {args[0]}", flush=True)
        subprocess.run(["docker", "run", "--rm", "--network", "container:" + cid,
                        "--read-only", "--tmpfs", "/run", "--cap-drop", "ALL", "--cap-add", "NET_ADMIN",
                        "--security-opt", "no-new-privileges:true", "--user", "0:0",
                        "--entrypoint", "python3", image, "-c", RULES, args[0], *blocked], check=True)


if __name__ == "__main__":
    main(sys.argv[1:])
