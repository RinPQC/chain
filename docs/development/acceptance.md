# M1 acceptance and operator handoff

M1 is a four-validator, classical-signature payment devnet. This document maps the completion criteria to reproducible tests and their limits. A passing trace is implementation evidence under the stated conditions, not a general consensus safety proof, a production readiness decision, or a post-quantum performance result. M2 has not started.

## Recorded baseline before WAL-first signing recovery

The original implementation backlog (#3–#14) is merged. [Tracker #15](https://github.com/RinPQC/chain/issues/15) remains open because an incomplete mandatory criterion blocks M1 completion. [Issue #30](https://github.com/RinPQC/chain/issues/30) tracks the remaining M1-04 signature/WAL recovery work; no recovery design is selected by this status update.

[Acceptance run 36957876702](https://github.com/RinPQC/chain/actions/runs/36957876702) passed both jobs for PR #29 head `7d043b4ad0e5c6501dea08241da8f1f46a75ba3a`. The artifact records the actual checked-out merge revision `faec6d0abfd00a32abd95b2178b2ef1e89656aab`. PR #29 was subsequently merged as `e34c6387b12111844a388664562b794d0c9584b6`.

| Observed result | Evidence |
| --- | --- |
| Native quality gate | 93 tests passed; two existing DNS tests skipped; initialization checks passed |
| 2–2 partition | All four heads remained at height 7 during the 12-second observation after settling |
| 3–1 partition | Majority advanced from height 11 to 14; isolated node remained at height 9 |
| Reconnection and restart | All four nodes converged at height 15, retaining payment balances, nonce and receipt |
| Finalized trace comparison | No conflicting block/state-root pairs across 15 retained heights; packet counters confirm cross-group drops |

The [devnet artifact](https://github.com/RinPQC/chain/actions/runs/36957876702/artifacts/11206801225) and [native artifact](https://github.com/RinPQC/chain/actions/runs/36957876702/artifacts/11207051016) expire after 30 days. These are results of that exact run, not a claim that every future revision or crash schedule passes. M1-01/02/03/05/06/07 have evidence within the matrix's declared bounds; M1-04 has planned-restart and partial crash-recovery evidence but retains the mandatory blocker.

The subsequent [WAL-first recovery implementation](signing-recovery.md) addresses #30 with an additional crash matrix and lock-replay tests. To complete the tracker, review that change and the full gate/container evidence for its resulting revision. Until then, do not close M1 or begin M2 on the basis of the merged initial backlog alone. Missing/corrupt recovery files, total rollback and copied active keys remain distinct unsupported conditions; addressing the ordinary process-crash window does not waive those checks.

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
```

The CI `check` job runs these Rust and initialization checks. The `devnet` job runs the actual container scenario on a clean Ubuntu 24.04 runner. YAML validation alone is not container-execution evidence.

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
| M1-04 restart and signing recovery | Native payment/restart and interrupted catch-up tests; decision-process-exit tests; `durable_signing_refuses_conflicts_regression_and_cross_chain_replay`; `signed_message_must_be_present_in_wal_before_restart_can_sign` | [WAL-first recovery](signing-recovery.md) adds `just recovery-check` (18 network crash cases with the recovered validator required for quorum) and authenticated-tail/lock-replay unit tests. This updates M1-04 beyond the baseline above; final acceptance requires review of the new run. Old stranded directories, lost engine history, total rollback and copied active keys remain unsupported |
| M1-05 fresh verified synchronization | Native fresh/returning test and container unused-validator catch-up; invalid certificate/execution cases in `sync_rejects_corruption_wrong_chain_bad_quorum_and_invalid_certified_execution` | Fixed membership and retained history; no arbitrary new validator, snapshots or pruning |
| M1-06 quorum loss, partitions and convergence | `two_validators_cannot_finalize_and_can_restart_their_active_wal`; actual 2–2 and 3–1 packet partitions in `smoke.py`; matching heads after both heals; conflicting height/block/root pairs fail the trace check | Eight seconds allow in-flight decisions to settle; 2–2 must then show an unchanged head on all four nodes for 12 seconds. This is a bounded observation, not a proof of indefinite non-finality |
| M1-07 reproducible operation | This clean-checkout walkthrough, linked native guides, `test_setup.py` and clean-runner container CI | Supported target is Linux x86-64 with Docker access. Native initialization can be tested without Docker; container scenarios cannot |

The decision-process-exit test drives the adapter recovery boundary with real persisted signed certificates; it does not inject a crash into every live engine actor instruction.

The matrix reports the demonstrated subset and the M1-04 limitation explicitly. It does not unilaterally declare M1 complete or waive the broader restart criterion; milestone closure is a separate review decision.

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

The companion `m1-native-<run-id>-<attempt>` artifact records the Rust toolchain, checked-out revision, complete quality-gate log and initialization checks. CI logs also record the container build. No keys, volume archives, signing journals or application databases are uploaded. Preserve artifacts outside CI before their retention expires if long-term evidence is required. A failed run may contain partial evidence; require both CI jobs to succeed for the same source revision and `report.json` to report success. The report checks all overlapping retained committed traces before container recreation; post-recreation agreement is checked through verified heads and payment state.

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

For ordinary shutdown, use SIGINT through the supplied controls. An expired stop grace period or host failure can still hit unsupported recovery windows. Consult [durable storage](storage.md) and [synchronization](synchronization.md) before interpreting `RECOVERED`, `SIGNING_READY` or `SYNC_VERIFIED`: only `COMMITTED` establishes local durable application inclusion.
