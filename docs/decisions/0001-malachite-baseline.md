# ADR 0001 — Malachite baseline for M1

Status: **Proposed for review; merge requires explicit owner confirmation.**

Date: 2026-10-01. Tracking: [issue #3](https://github.com/RinPQC/chain/issues/3).

## Problem and recommendation

M1 needs a reproducible BFT integration for a four-validator, one-coin payment network. The application must validate payments before supporting a proposal, commit finalized effects atomically, and recover without conflicting signing. Upstream API flexibility is useful only if these responsibilities remain explicit.

Recommend **Malachite v0.8.0 at commit `72143f6c99a98452b587e1c392bdb80944eb2232`**, using the **channel-based application interface**. Pin the Git revision initially so the dependency matches the inspected source exactly. Do not follow a moving branch, copy the engine into this repository, or modify its quorum/locking rules.

This record selects a proposed integration baseline, not production readiness. No upstream build, runtime benchmark, or security audit was performed for this issue. Application implementation begins in the dependent work items.

## Candidate comparison and provenance

| Candidate | Observation | Disposition |
| --- | --- | --- |
| Latest published GitHub release, `v0.8.0` | Published 2026-08-27; resolves to the revision above. | Preferred release baseline, pinned by commit. |
| Current `main` snapshot | GitHub reported the same revision on 2026-10-01. | No distinct newer implementation to prefer; never depend on the branch name. |
| Older `v0.7.0` release | v0.8.0 release notes identify subsequent WAL, recovery, and synchronization fixes and API changes. | Not selected; not independently audited or benchmarked in this comparison. |

The inspected [workspace manifest][manifest] declares version 0.8.0, edition 2021, **Rust 1.88 minimum**, and Apache-2.0. The root README's Rust 1.82 statement is stale relative to the manifest; use the manifest as the integration floor. Issue #5 must pin and validate an exact Rust toolchain and the resolved transitive dependencies before declaring a reproducible build.

The [README][readme] still calls the project alpha, under heavy development, and not externally audited. Formal specifications and model checking of parts of the engine are useful evidence, not a guarantee about our application.

The [release notes][release] and [breaking changes][breaking] make v0.8.0 particularly relevant: runtime WAL errors now propagate to a safety-stop path; round-certificate votes are persisted for recovery; synced values distinguish peer faults from local errors; and finalization evidence is preserved during height transitions. These are upstream descriptions corroborated by the inspected code paths, not reproduced test results.

## Actionable dependency specification

Use this entry when issue #5 creates the workspace:

```toml
[workspace.dependencies]
malachitebft-app-channel = { package = "arc-malachitebft-app-channel", git = "https://github.com/circlefin/malachite", rev = "72143f6c99a98452b587e1c392bdb80944eb2232" }
```

The package resides in `code/crates/app-channel`; Cargo discovers the upstream workspace under `code/`. Use its application re-exports where possible. Any additional direct Malachite dependencies must use this same Git URL and revision, rather than mixing registry and Git instances of the engine's types. Commit the resulting `Cargo.lock` and build with `--locked`. This snippet has been checked against source metadata, not compiled.

The channel crate's default feature set is empty. Its `byzantine` feature is for explicitly scoped test instrumentation; do not enable it by default. Select only required direct dependencies/features in issue #5 and record the resulting build requirements. The bundled Ed25519 implementation is a candidate for M1; transaction/key semantics remain issue #4/#6 work. No PQ library is selected here.

## Interface choice

The [architecture][architecture] offers three integration levels:

| Interface | Benefit | Tradeoff |
| --- | --- | --- |
| Channels | Application messages through Tokio channels, with the engine's networking, synchronization and recovery components. | Network/sync substitutions are not exposed as arbitrary plug-ins at this level. |
| Actors | Allows replacement and explicit wiring of engine actors. | More lifecycle and integration responsibility. |
| Core library | Consensus transitions independent of networking and I/O. | We must supply scheduling, transport, recovery and synchronization integration. |

Use channels for M1 to minimize infrastructure work. Keep application state transitions and wire types outside engine internals. Reconsider actors for M2 if PQ networking cannot be integrated through the selected interface; this is a known boundary, not a promise of a drop-in transport swap.

## Integration contract

The following names and obligations come from the pinned [application messages][messages], [context][context], [height parameters][height], and [signing interfaces][signing]. Use them rather than unversioned tutorials.

| Boundary | Engine behavior | RinPQC responsibility |
| --- | --- | --- |
| Initialization | Consumes application-defined types and per-height membership. | Authenticate genesis, restore state, provide the same four equal-weight validators, and establish safe signing readiness. |
| `GetValue` | Requests a proposal within a timeout. | Build a bounded, locally valid payment block; keep speculative effects separate from committed state. |
| `ReceivedProposalPart` | Delivers streamed parts and expects a complete proposed value when assembled. | Bound buffering, validate encoding and identity, reconstruct the block, execute all payment checks, and report its validity accurately before affirmative voting. |
| `StartedRound` / `RestreamProposal` | Requests previously seen values during recovery or re-proposal. | Retain/recover the necessary proposal material and return consistent value identities; do not substitute a new body under an old ID. |
| `Decided` | Supplies a commit certificate and waits for the application's acknowledgement before notifying sync of the decided height. | Atomically persist the decided block and payment effects, then acknowledge. Applying a repeated decision must be idempotent. |
| `Finalized` | Delivers the final certificate and observed equivocation evidence; awaits the application's next-height instruction. | Preserve required evidence and advance only after durable state is consistent. Additional signatures do not authorize changing the decided payment result. |
| `GetHistoryMinHeight` / `GetDecidedValues` | Requests application history; requested range bounds are an application obligation. | Serve bounded ranges of retained, verified blocks and the evidence required by the sync API. |
| `ProcessSyncedValue` | Requests application processing with `SyncedValueOutcome`. | Check decoded content and financial validity; distinguish `Verdict`, `PeerFault`, and `LocalTransientError`. A valid certificate alone is not proof of correct payment execution. |

Version 0.8.0 removes `PartsOnly`; use `ProposalAndParts` and its metadata/body binding. Do not copy older examples that depend on removed modes. Keep vote extensions disabled for the initial payment PoC; if later required, their authenticated content and persisted extended certificates need a separate application contract.

`HeightParams.target_time` and consensus round timeouts are separate controls. Explore a roughly five-second target without treating timeouts as a slot schedule. `Decided` and `Finalized` may be separated by the finalization period. Issue #4 must specify the exact RPC success event; neither local admission nor the end of a timer establishes successful settlement.

## Security envelope and proposer selection

Retain the default strict **greater-than-two-thirds voting-power quorum** from [threshold.rs][threshold]. Four validators with weight one require three votes; two cannot meet the quorum. The Tendermint model assumes less than one-third Byzantine voting power, appropriate cryptographic authentication, and network recovery/timeliness conditions for liveness. This is a fixed-membership development network, not a Sybil-resistance mechanism.

`Context::select_proposer` is application-provided. Recommend deterministic rotation over a canonical validator ordering, incorporating both height and round so an unavailable proposer does not remain the only candidate. Define exact indexing in issue #4 and test eventual selection of a correct proposer in issue #9. No extra cooldown, sampling, randomness beacon, or altered locking rule is introduced.

[Commit certificates][certificate] contain height, round, value ID and individual validator signatures. They are not a compact cryptographic aggregate. Use the [certificate verification helpers][verification] with the authenticated validator set and ensure malformed/duplicate evidence cannot inflate voting power. Changing cryptography changes evidence sizes and verification work; no M1 size assumption is a PQ capacity claim.

## Durability and safe restart

The [engine consensus path][engine] flushes WAL state before publishing consensus messages and before delivering decisions. The [node supervisor][node] distinguishes safety-critical WAL failures from ordinary failures: a safety failure enters a safety-hang state and exposes `node_safety_failure`, requiring operator attention rather than blind automatic recovery.

The application database is not made transactional by the engine WAL. Issue #8 must define the joint recovery contract: committed block/height, both balances, transfer status and replay state must agree, while consensus signing history must not roll back. Test crashes before and after each acknowledgement boundary. Never delete a WAL or restore stale signing state merely to make the node start.

Crash restart with intact durable state and disaster recovery from missing/stale signing metadata are different cases. The latter must fail closed until safe readiness is established. The devnet supervisor must not restart around the engine's safety-hang as though it were a normal liveness failure. Validate these behaviors in issues #9, #11 and #14.

## Cryptography and PQ boundaries

[SigningScheme][scheme] abstracts signature/public/private-key types and encoding; the separate signer/verifier interfaces perform signing and verification. These are suitable integration seams for M2, subject to compatibility testing. The private-key associated type currently requires `Clone`: a chosen PQ library with deliberately non-cloneable secret types will need a reviewed ownership/handle adapter rather than an assumption that its types fit directly.

Specify chain and role domain separation in our canonical signing messages. Do not assume the trait supplies network binding automatically. Give transaction authorization, consensus signing and network identity separate responsibilities even if M1 uses the same signature family.

The bundled [network implementation][network] uses libp2p identities and TCP/Noise or QUIC transport paths. A custom consensus signer does not replace those dependencies. M2 must examine peer authentication, transport, resource limits and any required network-actor replacement independently. Reuse does not establish PQ security for the complete node.

## Evidence gaps and downstream gates

| Gap / required evidence | Owning work item |
| --- | --- |
| Exact Rust toolchain, dependency resolution, platform prerequisites and a successful locked build | [#5](https://github.com/RinPQC/chain/issues/5) |
| Deterministic formats, signing domains, genesis identity, proposer indexing and outcome semantics | [#4](https://github.com/RinPQC/chain/issues/4), [#6](https://github.com/RinPQC/chain/issues/6) |
| Application validity cannot be bypassed through streamed proposals or synchronization | [#7](https://github.com/RinPQC/chain/issues/7), [#9](https://github.com/RinPQC/chain/issues/9), [#11](https://github.com/RinPQC/chain/issues/11) |
| Atomic application commits and durable signing recovery across injected failures | [#8](https://github.com/RinPQC/chain/issues/8), [#14](https://github.com/RinPQC/chain/issues/14) |
| Correct handling of timeout caps, partitions, faulty proposers, safety-hang and live catch-up | [#13](https://github.com/RinPQC/chain/issues/13), [#14](https://github.com/RinPQC/chain/issues/14) |
| PQ signer compatibility, independent network-crypto replacement and measured resource costs | M2; not claimed by this baseline |

No unresolved source-level blocker was identified for starting the M1 integration work. Build compatibility and behavior remain unverified until the assigned tasks produce evidence. Revisit this record if they invalidate the selected interface or revision.

## Validation and change control

Compared the GitHub release metadata and `main` revision, checked out v0.8.0, and inspected manifests, release/breaking notes and the linked source paths. No compilation, execution or performance measurement is claimed. All source references below pin the same inspected commit.

Upstream updates require a dedicated PR examining behavior, data/wire compatibility and recovery. After review is complete, notify the owner and wait for explicit merge permission. Authorized merges use squash. Completing this issue does not authorize silent changes to M1 scope or start the next task before the agreed review gate.

[manifest]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/Cargo.toml
[readme]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/README.md
[release]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/RELEASE_NOTES.md
[breaking]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/BREAKING_CHANGES.md
[architecture]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/ARCHITECTURE.md
[messages]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/app-channel/src/msgs.rs
[context]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-types/src/context.rs
[height]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-types/src/height_params.rs
[signing]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/signing/src/lib.rs
[threshold]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-types/src/threshold.rs
[certificate]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-types/src/certificate.rs
[verification]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/signing/src/ext.rs
[engine]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/engine/src/consensus.rs
[node]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/engine/src/node.rs
[scheme]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-types/src/signing.rs
[network]: https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/network/src/lib.rs
