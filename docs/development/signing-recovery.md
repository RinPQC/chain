# WAL-first signing recovery

This change addresses #30 without replacing Malachite or rewriting engine history. It changes the application's signature-release ordering and reconstructs only an authenticated missing tail of its signing journal. The engine remains pinned to `72143f6c99a98452b587e1c392bdb80944eb2232` (0.8.0). The implementation and evidence are subject to review; this document does not independently close M1.

## Problem and invariant

Previously, the signer committed a signature to the application journal and returned it to Malachite. The engine then appended the corresponding input to its WAL. A process exit in between left a durable signing commitment without the engine replay record, so startup correctly refused to sign.

The new order for a previously unseen signing slot is:

1. Serialize host and engine signing calls; check the durable parent height, immutable slot and high watermark before changing WAL state.
2. Append the exact signed proposal/vote through the engine's existing WAL actor. Wait for append acknowledgement and then an explicit successful flush. A host-built proposal's block bytes are already durable before this point.
3. Commit the same signed bytes and watermark to the application signing journal.
4. Return the signature to the caller. The engine's normal processing may append the same input again and flush before broadcast, as before.

A returned signature therefore already has both durable records. WAL contains the causal inputs preceding a vote as well as the exact local signed message; recovery never tries to infer the engine's locks from a signature alone. Existing identical signed slots are reused only after startup has checked their correspondence with WAL. Any WAL write/flush failure latches signing closed. The host tracks started-height notifications to prevent a call at an uninitialized WAL height; the pinned WAL actor otherwise acknowledges mismatched-height appends without writing.

The actor owns the WAL file throughout normal operation. The application neither opens a second writer nor inserts entries into an offline log during recovery. Replay is performed by the unchanged consensus engine. Normal shutdown still drains consensus and flushes WAL.

## Why input ordering matters

In the pinned [vote handler](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-consensus/src/handle/vote.rs), an accepted vote is appended before applying it to the driver. [Proposed values](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-consensus/src/handle/proposed_value.rs), [timeouts](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-consensus/src/handle/timeout.rs) and [liveness certificates](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-consensus/src/handle/liveness.rs) likewise persist relevant inputs before their state transitions. The [driver output handler](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/core-consensus/src/handle/driver.rs) requests a signature before routing the resulting local message through the normal input handler.

The signer-side append occupies that same position for engine-generated output. Its flush also makes preceding causal inputs durable. For a host-generated proposal, the signed proposal can precede its proposed-value input, as it can for a remote proposal; its independently validated block is durably cached and the recovered part is supplied through the ordinary host path. No vote is authorized merely by possessing that part.

The [runtime replay loop](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/engine/src/consensus.rs) feeds WAL inputs back through consensus and suppresses ordinary WAL writes during replay. Exact previously signed requests return cached commitments; conflicting or regressed requests fail. The new release barrier must also apply to any genuinely new signing request. Duplicate identical local inputs remain subject to the normal engine verification and deduplication behavior.

## Restart reconciliation

Before constructing the enabled signer, startup verifies application history and certificates, then inspects WAL and the signing journal:

- WAL must exist and its sequence must match the committed or immediately next height. WAL entry decoding/checksums must pass.
- Local WAL signatures and every journaled signature must authenticate under the configured chain and validator identity. Journal key/height/round/phase and the high watermark must agree.
- Every active journal commitment must already occur in WAL. Missing engine history is never manufactured from the journal.
- Only directories marked `recovery_policy = wal-first-v1` may copy a WAL-only active signature into the journal. Such entries must form a nonconflicting tail strictly above its existing watermark.
- Active local proposals require the durable block payload, valid signature, expected proposer and successful execution against the verified parent. Validate all proposed repairs before making any recovery write.
- Apply the missing journal tail and restore missing proposal parts idempotently, then run strict correspondence checks again.

Legacy directories have no policy marker. They must pass the old strict correspondence check before the marker is installed and WAL-first signing begins. A directory already stranded by the old journal-before-WAL window remains rejected: its absent engine history cannot be reconstructed safely by this change. No keys, WAL records or history are reset for migration.

A process exit after WAL flush but before journal commit leaves a recoverable WAL-only tail. An exit after journal commit leaves matching records. An exit during reconciliation can be retried. Incomplete unpublished WAL tail writes remain subject to the pinned WAL reader's partial-tail handling; missing durable commitments, decode/checksum failures and contradictory state still refuse startup.

This assumes intact local storage honoring flushes and exclusive ownership of the validator key. It does not detect a coordinated rollback of all consistent files, repair lost causal history, or make concurrent copies of a validator key safe. A corrupted or adversarially rewritten storage device is outside the crash-recovery guarantee.

## Verification

The normal gate includes recovery-unit tests for authenticated journal-tail reconstruction, repeated reconciliation, legacy rejection, invalid signatures, conflicting slots, wrong heights, checksum corruption, missing WAL, and missing or invalid proposal payloads. Existing immutable-slot, chain-binding, storage-atomicity and verified-sync tests remain required.

The opt-in `fault-injection` feature adds subprocess exit points absent from default builds and the Docker image. It must never be enabled for ordinary operation. The network fixture waits for all three running nodes to have their gossip subscriptions established before releasing the first consensus round; it does not depend on process startup timing. The network test crosses proposal/prevote/precommit, before-WAL/after-WAL/after-journal exits and rounds zero/one. It also interrupts journal and proposal-part recovery, preserves previous signed bytes, checks payment state and requires the recovered validator to restore a three-of-four quorum while the fourth stays offline. Precommit cases compare the finalized block with the value locked before the crash. A separate core-replay test checks that a later-round conflicting proposal cannot erase the recovered lock. Two remaining validators alone cannot be assumed to force the required round transition.

These are finite process-exit traces, not a power-loss proof, arbitrary Byzantine execution proof or PQ performance claim. Run `just check` first, then `just recovery-check`. The latter builds with `fault-injection`, lints that configuration and runs the crash matrix. It changes the local debug binary; rebuild with `cargo build --locked -p rinpqc-node` before ordinary operation. Do not run default and feature-enabled builds concurrently against the same target directory while subprocess tests are active. CI runs these steps sequentially and retains `signing-recovery.log` in its native evidence artifact. Exact observed results accompany the implementation PR.
