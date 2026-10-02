# M1 acceptance and operator handoff

M1 is a four-validator, classical-signature payment devnet. This document maps the completion criteria to reproducible tests and their limits. A passing trace is implementation evidence under the stated conditions, not a general consensus safety proof, a production readiness decision, or a post-quantum performance result. M2 has not started.

## Accepted M1 evidence

The implementation backlog (#3–#14) and the final M1-04 recovery task (#30) are merged. [PR #32](https://github.com/RinPQC/chain/pull/32) was approved by redlucy57 on head `fb1060ce8adb44402603580f6472db9f2b095826` and merged as `59eded100dd5cdab153b47b2f3cbaaac3dc40865`. Together with the evidence below, this closes the mandatory criteria M1-01–M1-07 within their declared PoC bounds. [Tracker #15](https://github.com/RinPQC/chain/issues/15) records completion; M2 has not started.

[Acceptance run 36968054144](https://github.com/RinPQC/chain/actions/runs/36968054144) passed both jobs for that PR head. Its actual tested merge checkout was `825761f841c9e76f19565624629ed0f3571e7e81`. The merge commit is a separate identifier; both revisions have the identical Git tree `b756a8b95fb1f1813ba0c26a2d1a2fd877bf907d`. These observations describe the recorded test checkout.

| Observed result | Evidence |
| --- | --- |
| Native quality gate | 98 tests passed; two existing DNS tests skipped; initialization, lint, formatting and dependency checks passed |
| Signing crash recovery | All 18 cases passed in 332.9 seconds: proposal/prevote/precommit, before-WAL/after-WAL/after-journal, rounds zero/one; repeated recovery interruption and recovered validator required for quorum |
| Lock preservation | Core replay rejects a conflicting later-round proposal while retaining the recovered lock; network precommit cases retain the locked block |
| 2–2 partition | All four heads remained at height 6 during the 12-second observation after settling |
| 3–1 partition | Majority advanced to height 11; isolated node remained at height 8 |
| Reconnection and restart | Reconnection converged at height 12; persistent restart converged at height 13 with matching payment state |
| Finalized trace comparison | No conflicting block/state-root pairs across 12 retained pre-recreation heights; post-recreation agreement checked separately |

The [devnet artifact](https://github.com/RinPQC/chain/actions/runs/36968054144/artifacts/11210318219) and [native artifact](https://github.com/RinPQC/chain/actions/runs/36968054144/artifacts/11210268909) expire after 30 days. These are results of that exact run, not a claim that every future revision or crash schedule passes. The earlier [PR #29 baseline](https://github.com/RinPQC/chain/actions/runs/36957876702) had 93 passing tests and left M1-04 incomplete; it is superseded by the reviewed recovery evidence above.

[WAL-first recovery](signing-recovery.md) removes the old journal-before-WAL release window and recovers authenticated missing journal tails from intact engine history. Already-stranded legacy directories, missing/corrupt recovery files, total rollback and copied active keys remain unsupported. M1 acceptance neither waives those checks nor establishes production or post-quantum readiness.

## Reproduce from a clean checkout

On Linux x86-64, install Git, Python 3, Docker Engine and Compose v2, with permission to use the Docker daemon. Clone the repository and select the exact commit attached to the acceptance run:

```sh
git clone https://github.com/RinPQC/chain.git
cd chain
git checkout <tested-commit>
docker compose up --build -d
scripts/devnet.sh status
scripts/devnet.sh pay 30
```

The four containers generate separate validator and transport keys and a shared genesis. The funded transaction key exists only in node0's volume. Keys are disposable and are never part of the evidence artifact. Read the `tx_id`, `sender`, `recipient` and `signed_file` from the payment response, then:

```sh
scripts/devnet.sh rpc node0 transaction <tx_id>
scripts/devnet.sh rpc node0 account <sender>
scripts/devnet.sh rpc node0 account <recipient>
scripts/devnet.sh retry <signed_file>
scripts/devnet.sh restart node1
scripts/devnet.sh logs node1
scripts/devnet.sh down
scripts/devnet.sh up
```

Wait for `finalized`: the first payment leaves sender balance `3999970`, sender nonce `1` and recipient balance `30`. The identical retry must retain the receipt without a second debit. Ordinary down/up retains keys and history. A healthy RPC endpoint alone does not establish progress or agreement; compare chain ID, height, block ID and state root across nodes.

For native installation, manual key creation and offline signing, follow [build](build.md), [network initialization](consensus.md) and [payment CLI](rpc.md), in that order. These paths use the same node binary and payment rules as Compose.

For a complete destructive acceptance run, use a separate, **fresh** project:

```sh
COMPOSE_PROJECT_NAME="rinpqc-test-$(date +%s)" python3 scripts/devnet/smoke.py
```

This builds and starts a real network, exercises the scenarios below, records evidence in `artifacts/m1/`, and deletes only that test project's containers, bridge and four volumes. It refuses an existing volume set. Stop any devnet using the same subnet first, or set a different `DEVNET_PREFIX` as described in the [devnet guide](devnet.md). Do not run competing acceptance jobs with the same project name or evidence directory.

Install the pinned native build tools from [build.md](build.md) and run:

```sh
just check
just devnet-check
just recovery-check
```

The CI `check` job runs these Rust, initialization and signing-crash checks sequentially. `just recovery-check` builds a test-only fault-injection binary; rebuild with `cargo build --locked -p rinpqc-node` before normal operation. The `devnet` job runs the actual container scenario on a clean Ubuntu 24.04 runner. YAML validation alone is not container-execution evidence.

## Declared configuration and assumptions

| Item | Acceptance configuration |
| --- | --- |
| Node / compiler | `rinpqc-node 0.1.0`, Rust 1.93.0; exact source revision in `report.json` |
| Engine | Malachite 0.8.0, revision `72143f6c99a98452b587e1c392bdb80944eb2232`; patches in `vendor/README.md` |
| Dependencies / image | Committed `Cargo.lock`; base image digests in `Dockerfile`; Debian packages resolved at build time, so not a bit-reproducible image claim |
| Membership / quorum | Four fixed, equal-weight genesis validators; three distinct precommits required |
| Crypto | Classical Ed25519 authorization and transport identities; no PQ claim |
| Limits | Five-second target interval, 1 MiB blocks, 4096 transfers/block and 4096 pending entries; fresh genesis records actual limits |
| Network | One machine, private IPv4 Docker bridge, static peer addresses, no host-published RPC ports; no simulated WAN latency or load |
| Persistence | Four separate local volumes; filesystem/device must honor flushes; no rollback or duplicated active validator keys |
| Synchronization | Full retained blocks and certificates from genesis; fresh join means an unused genesis validator, not new membership |
| Faults | Controlled SIGINT, explicit process-exit boundaries, absent proposer, packet-drop partitions and reconnect; no arbitrary Byzantine scheduler or faulty-disk simulation |

Three correct available validators are expected to progress once their mutual communication and processing remain timely, including after proposer timeouts. A five-second target is not a failure-time deadline. Safety relies on the engine's quorum/locking assumptions, less than one-third Byzantine voting power and preserved signing state; the fixed devnet does not solve permissionless admission or copied-key protection.

## Acceptance matrix

Test names below are stable selectors for `cargo nextest run --locked -p rinpqc-node -E 'test(NAME)'`. Run the full gate for acceptance; a selected passing test is not the whole milestone.

| Criterion | Evidence / checks | Limits or blocker |
| --- | --- | --- |
| M1-01 identical history and balances | `four_validators_commit_payments_restart_and_replace_missing_proposer`; `fresh_and_returning_validator_resume_verified_history_while_peers_keep_producing`; container scenario compares heads, balances and every retained `COMMITTED` trace at overlapping heights | Finite traces on one host; not a proof for all schedules |
| M1-02 atomic, exactly-once payments and rejection | `invalid_payments_and_concurrent_double_spend_have_one_durable_winner` sends invalid signatures and insufficient-funds payments through real RPC, races two valid nonce-zero spends at different nodes, checks one receipt and all affected balances/nonces before and after restart; `payment_cli_submits_finalizes_and_retries_without_a_second_debit`; `process_exit_at_each_commit_boundary_recovers_all_or_nothing`; `decision_process_exit_recovers_payment_only_with_durable_certificate` | A losing conflicting request may initially be pending on another node; only one can finalize. The winning identical retry returns the original receipt rather than an error. Process exit does not emulate power loss inside the storage driver |
| M1-03 one validator/proposer unavailable | Native missing-proposer test and container `fresh`: start the three validators excluding the scheduled initial proposer; 3–1 scenario requires each majority node to advance by two blocks | Requires timely communication among three correct validators; no throughput guarantee |
| M1-04 restart and signing recovery | Native payment/restart and interrupted catch-up tests; decision-process-exit tests; `durable_signing_refuses_conflicts_regression_and_cross_chain_replay`; `signed_message_must_be_present_in_wal_before_restart_can_sign` | [WAL-first recovery](signing-recovery.md) adds `just recovery-check` (18 network crash cases with the recovered validator required for quorum) and authenticated-tail/lock-replay unit tests. The reviewed run above satisfies M1-04 within the intact-storage process-crash scope. Old stranded directories, lost engine history, total rollback and copied active keys remain unsupported |
| M1-05 fresh verified synchronization | Native fresh/returning test and container unused-validator catch-up; invalid certificate/execution cases in `sync_rejects_corruption_wrong_chain_bad_quorum_and_invalid_certified_execution` | Fixed membership and retained history; no arbitrary new validator, snapshots or pruning |
| M1-06 quorum loss, partitions and convergence | `two_validators_cannot_finalize_and_can_restart_their_active_wal`; actual 2–2 and 3–1 packet partitions in `smoke.py`; matching heads after both heals; conflicting height/block/root pairs fail the trace check | Eight seconds allow in-flight decisions to settle; 2–2 must then show an unchanged head on all four nodes for 12 seconds. This is a bounded observation, not a proof of indefinite non-finality |
| M1-07 reproducible operation | This clean-checkout walkthrough, linked native guides, `test_setup.py` and clean-runner container CI | Supported target is Linux x86-64 with Docker access. Native initialization can be tested without Docker; container scenarios cannot |

The decision-process-exit test drives the adapter recovery boundary with real persisted signed certificates; it does not inject a crash into every live engine actor instruction.

All seven criteria have reviewed implementation evidence within the limits recorded here. This acceptance covers a fixed four-validator devnet and finite fault traces; it does not establish permissionless operation, arbitrary Byzantine-schedule safety, mobile performance or PQ security.

## Partition controls and evidence

```sh
scripts/devnet.sh split 2-2
scripts/devnet.sh fault-status
scripts/devnet.sh heal
scripts/devnet.sh split 3-1
scripts/devnet.sh fault-status
scripts/devnet.sh heal
```

`2-2` keeps node0/node1 together and node2/node3 together. `3-1` keeps node0/node1/node2 together and isolates node3. Cross-group packets are dropped by source IP in each validator's network namespace; within-group links and loopback RPC remain available. Existing TCP connections are affected as well as new dials. All four nodes must be running and attached to their original bridge. Do not combine these rules with the separate bridge-disconnect commands.

The helper verifies Compose project/service ownership and uses the target container's exact image ID. It runs a temporary root container with only `NET_ADMIN`, no host mounts, a read-only root filesystem and a writable temporary `/run`, sharing only that validator's network namespace. Validators themselves retain their unprivileged configuration. The helper does not join the host network or install host firewall rules. `heal` removes only the `RINPQC_TEST` chain and its INPUT jump. If applying a split fails partway, run `heal` before continuing; never interpret a partially applied split as an acceptance result. Container recreation also destroys these namespace-local rules.

The CI artifact `m1-devnet-<run-id>-<attempt>` is retained for 30 days and contains:

- `report.json`: source revision, host platform/CPU count, Docker/Compose versions, scenario head snapshots, packet-rule counters, payment public IDs and success status.
- `compose.yaml` and `genesis.json`: resolved topology, operational configuration and public genesis.
- Per-node pre-recreation logs and final-container logs: committed height/block/state-root traces, recovery and synchronization observations.

The companion `m1-native-<run-id>-<attempt>` artifact records the Rust toolchain, checked-out revision, complete quality-gate log, initialization checks and `signing-recovery.log` with all 18 crash cases. CI logs also record the container build. No keys, volume archives, signing journals or application databases are uploaded. Preserve artifacts outside CI before their retention expires if long-term evidence is required. A failed run may contain partial evidence; require both CI jobs to succeed for the same source revision and `report.json` to report success. The report checks all overlapping retained committed traces before container recreation; post-recreation agreement is checked through verified heads and payment state.

## Recovery and troubleshooting

| Observation | Action |
| --- | --- |
| Build cannot reach registries or Rust sources | Restore network access; keep lockfile/pins, then rebuild. This is not a consensus failure |
| Docker permission denied | Use an account authorized for the daemon; native tests remain available. Do not make the socket world-writable |
| Bridge subnet overlaps an existing network | Stop the old devnet or use another `DEVNET_PREFIX` with a fresh project |
| RPC healthy but height stalls | Check three mutually connected nodes, round/proposer logs, partition rules and chain identity; use `heal` only for the test rules |
| Submission times out or is pending after restart | Query its transaction ID and retry the identical signed file; do not infer failure or change nonce solely from timeout |
| Partial initialization, missing WAL, signature/WAL mismatch, conflicting history or corrupt state | Stop the affected identity and preserve all files. Do not delete the WAL, import balances alone, or start a copied key. No supported general repair exists |
| Disposable network is no longer needed | `scripts/devnet.sh reset --discard-test-state`; this irreversibly removes all four volumes, including keys and balances |

For ordinary shutdown, use SIGINT through the supplied controls. An abrupt process exit with intact, durably flushed files follows the WAL-first recovery path. Power loss with dishonest flushes, lost files or rollback remains outside this guarantee. Consult [durable storage](storage.md) and [synchronization](synchronization.md) before interpreting `RECOVERED`, `SIGNING_READY` or `SYNC_VERIFIED`: only `COMMITTED` establishes local durable application inclusion.
