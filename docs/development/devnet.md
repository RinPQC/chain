# Disposable four-validator devnet

This is the M1 payment chain with four fixed, equal-power validators, one native test coin and classical signatures. Generated keys, balances and history are disposable test material. It is not a public testnet, staking network, production deployment or a hardware-performance benchmark.

## Prerequisites and first launch

Use Linux x86-64, Docker Engine, Docker Compose v2 and Bash. Your account must be able to reach the Docker daemon. Building downloads the pinned Rust/Debian images, locked dependencies and native packages; it requires network access and sufficient compiler memory/disk space. No host Rust installation is needed. Base-image digests and Cargo dependencies are pinned; OS package repositories remain an external build input, so this is repeatable setup rather than a bit-for-bit build claim.

From the repository root:

```sh
docker compose up --build -d
scripts/devnet.sh status
scripts/devnet.sh logs node0
```

The initial image build takes longer than later starts. The initializer creates fresh random keys, shared genesis and bound recovery data. Each validator has its own named volume and distinct consensus/RPC ports. Only node0 receives the disposable funded payment key (4,000,000 native test units). The initializer can access all four volumes during setup; running validators can access only their own volume. Processes run as UID/GID 10001, without capabilities and with a read-only root filesystem. Consensus ports 31001–31004 remain on the Docker bridge. RPC ports 32001–32004 bind only to each container's loopback. No ports are published to the host.

A default project uses the bridge `rinpqc-devnet_consensus` and volumes `rinpqc-devnet_node0` through `rinpqc-devnet_node3`. To run a separate devnet or avoid a conflicting subnet, set both variables **before initialization** and keep them for all commands:

```sh
export COMPOSE_PROJECT_NAME=rinpqc-lab
export DEVNET_PREFIX=172.30.92
```

The default prefix is `172.30.91`, forming a `/24` bridge with validator addresses `.10`–`.13`. Changing the prefix of an existing devnet is refused; preserve the old settings or explicitly reset disposable state. Only use a valid, unused IPv4 subnet.

Health means the node's local RPC can read verified state. It does not mean quorum exists, finality is advancing or synchronization has reached the newest peer height. Compare committed heights over time and inspect rounds/logs for those conditions.

## Submit and inspect a payment

```sh
scripts/devnet.sh pay 30
```

The helper creates a fresh recipient test key on node0, reads the funded account's nonce, signs locally and submits the resulting file. Output contains `tx_id`, `sender`, `recipient`, `signed_file` and the submission response. Copy those public values into these commands:

```sh
scripts/devnet.sh rpc node0 transaction <tx_id>
scripts/devnet.sh rpc node1 account <recipient>
scripts/devnet.sh retry <signed_file>
```

Wait for `finalized` before treating the payment as settled or starting another payment from the same funded account. `pending` is a volatile local queue entry. Retrying the same file is safe before/after finalization and after a normal restart; it never authorizes a second debit. Timeout leaves the outcome uncertain: query the same ID or retry the same file. The raw signing/API rules remain in the [RPC guide](rpc.md).

## Ordinary stop, restart and shutdown

```sh
scripts/devnet.sh stop node1
scripts/devnet.sh start node1
scripts/devnet.sh restart node2
scripts/devnet.sh down
scripts/devnet.sh up
```

Compose sends SIGINT, allowing the host to stop at a message boundary. Data volumes and identity files survive all these commands. `up` reuses initialized data and checks its chain/identity binding; it never silently regenerates keys or repairs partial initialization. Restarted nodes use their retained WAL/signing history and verify missed blocks. The existing fail-closed signature-before-WAL crash window still applies to hard kills or host failure: synchronization is not permission to bypass an unsafe signing history. See [recovery limits](synchronization.md).

## Initial proposer outage and fresh-validator catch-up

Use a new project/volumes for this scenario, or consciously perform the explicit reset below first:

```sh
scripts/devnet.sh fresh
scripts/devnet.sh first-proposer
scripts/devnet.sh status
```

`fresh` initializes all four identities but starts only three validators, withholding the first scheduled proposer. The three active nodes must advance to a later round and can finalize with quorum. The command prints which node was withheld. After the active nodes have committed several blocks, start that unused validator:

```sh
scripts/devnet.sh start node2  # Example only: use the node printed by fresh.
scripts/devnet.sh logs node2
scripts/devnet.sh rpc node2 metrics
```

Look for `SYNC_VERIFIED`, increasing `committed_height` and matching block IDs at the same height. The script refuses `fresh` if any identity was already started; it never deletes a used validator's history to simulate a fresh node. Subsequent `stop`/`start` exercises a returning validator instead. A current proposer can also be taken offline with `stop` after matching the public `proposer` metric to the validator IDs in the `ROUND` logs; selection may move before the stop takes effect.

## Partition and reconnect

```sh
scripts/devnet.sh partition node0 node1
scripts/devnet.sh status
scripts/devnet.sh reconnect node0 node1
```

Disconnecting a node removes its consensus bridge interface while preserving the process, volume and loopback RPC. These commands isolate node0 and node1 individually, leaving only node2/node3 connected: no component has three validators, so new finality must stop after already certified/in-flight decisions settle. This is isolation, not simulated packet loss or a two-by-two topology. Reconnect restores the original static addresses; peers retry and verified synchronization repairs missed history. No firewall rules or host networks are modified.

Inspect every node's metrics before and after reconnect. Increasing rounds without increasing committed height indicate unavailable finality; an RPC healthcheck alone cannot detect that. A command against an already disconnected/connected node can fail visibly; inspect the network state rather than resetting data to clear it.

## Metrics and logs

```sh
scripts/devnet.sh rpc node0 metrics
scripts/devnet.sh rpc node0 chain-status
scripts/devnet.sh logs node0
```

Metrics are version-1 local RPC JSON, not a Prometheus HTTP endpoint. Numeric fields are decimal strings. Counters are per process, saturate at their integer bound and reset on restart; they never affect consensus decisions. `committed_height` and `queue_depth` are read from the same host-owned state as payment RPC.

| Field | Meaning |
| --- | --- |
| `uptime_seconds` | Elapsed time since diagnostics were initialized after recovery |
| `recovered_height`, `recovery_milliseconds` | Verified startup checkpoint and local history/WAL validation time |
| `consensus_height`, `round`, `proposer` | Last started round, with the public proposer ID; round is `-1` before one starts |
| `committed_height` | Current durable application height, including verified catch-up |
| `finalized_total` | New durable decisions processed since startup, including sync; excludes recovered historical commits |
| `queue_depth` | Current admitted, volatile payment count |
| `rejected_proposals_total` | Sequence-zero proposal parts rejected by host validation/window/capacity policy; not all transport/engine rejections |
| `sync_verified_total` | Host-verified sync payload callbacks, not a finality or byte-throughput counter |
| `sync_rejected_total` | Peer-fault sync payload callbacks rejected by the host; engine-level wire/certificate rejections are excluded |

`RECOVERED`, `SIGNING_READY`, `ROUND`, `SYNC_VERIFIED`, `COMMITTED` and `PROPOSAL_REJECTED` identify the relevant stage and height/round. Upstream warning/error logs describe connection failures and invalid network traffic. Neither metrics nor these logs include secret keys or seeds. Compose prefixes log lines with the service name and rotates each node's JSON logs at 10 MB with three files. Match both chain ID and height/block ID when comparing different networks or restarts.

## Explicit destructive reset

```sh
scripts/devnet.sh reset --discard-test-state
```

This removes this Compose project's containers, bridge and **all four named volumes**, including test keys, signed payment files, balances, WAL and complete history. The command names the project and volumes before removal. Omitting `--discard-test-state` refuses the operation. A subsequent `up` creates new keys and a new chain ID. Do not point this disposable project at valuable data or use its private test keys elsewhere.

## Validation

`just check` verifies Rust behavior, including real subprocess consensus, RPC payment/retry and diagnostic observations of sync, finalization, queue depth and recovery. `just devnet-check` requires Python 3 and exercises actual key/config creation, stable re-initialization, partial-state rejection and the unused-identity guard without Docker.

CI additionally builds the actual image and runs `scripts/devnet/smoke.py`: proposer outage, unused-validator catch-up, matching heads, payment/retry, restart, loss/restoration of quorum, ordinary down/up with retained receipts and explicit volume deletion. This script requires a fresh `COMPOSE_PROJECT_NAME` beginning with `rinpqc-test-` because it destroys the test volumes. Container validation requires access to a Docker daemon; successful YAML validation alone is not evidence that the devnet ran. Wider Byzantine/load acceptance work remains #14.
