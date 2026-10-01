# Durable payment state

`storage::Store` persists the M1 application ledger. A transaction writes the canonical block, both sides of every transfer (including nonces), inclusion receipts and the committed head together. Nothing is published to the in-memory ledger until the durable commit succeeds. There are no fees, minting, pruning or snapshot imports.

This is a library boundary, not a running consensus node. `commit_decided(height, bytes)` **requires a trusted consensus caller that has authenticated a decision for these exact block bytes**. It rechecks execution but does not verify a quorum certificate. Do not connect it directly to RPC or feed it unverified synchronization data. A stored receipt proves local inclusion under that trust boundary; it is not a portable finality proof.

## Storage choice and durability

The initial backend is pinned `redb 4.3.0` (MIT OR Apache-2.0, Rust 1.90 minimum). It provides embedded transactions and an exclusive database lock without a native database build. SQLite with explicit durable transactions would also fit; RocksDB would add native build and operational configuration for capabilities this small PoC does not yet need. The storage API isolates this implementation choice from payment semantics. This choice makes no production-throughput or mobile-suitability claim.

Every initialization and application write explicitly uses `Durability::Immediate` and two-phase commit. Immediate durability completes before a successful return; two-phase commit adds a data flush before activating the new commit slot. See the pinned upstream [durability contract](https://docs.rs/redb/4.3.0/redb/enum.Durability.html) and [two-phase commit description](https://docs.rs/redb/4.3.0/redb/struct.WriteTransaction.html#method.set_two_phase_commit). These guarantees depend on the filesystem and storage device honoring flushes. A process-exit test is not a power-loss or faulty-device proof.

`Store::create` exclusively creates a new file (0600 on Unix), commits genesis and syncs the parent directory on Unix. `Store::open` requires an existing database and never silently creates a replacement. Keep the file in a private, trusted node data directory on a local filesystem; its ancestors must not be writable by untrusted users. The database lock excludes another store on the same file, but does not coordinate all node files or protect a copied validator key.

A missing, empty, partially initialized, incompatible or inconsistent database must stop startup. Do not delete it and regenerate a signing node automatically. Initialization does not constitute signer initialization. The configured database cache is 8 MiB; this is not a bound on total process memory.

## Local data format

Application schema version 1 uses one redb byte-key/byte-value table, `application_v1`. Integers are unsigned big-endian; balances and nonces retain the full u64 range. The backend's own file format is versioned separately.

| Key | Value |
| --- | --- |
| `version` | Application schema u32, currently 1. |
| `genesis` | Exact canonical genesis bytes, binding chain identity and all limits. |
| `head` | Height u64, block ID (32 bytes), state root (32 bytes). At height zero the block ID is the genesis chain ID. |
| Byte `b` + height u64 | Complete canonical block bytes. Every committed height is retained. |
| Byte `a` + account ID | Balance u64 and next nonce u64. Historical zero-balance accounts remain present. |
| Byte `r` + transaction ID | Height u64, index u32, block ID, sender ID, recipient ID, sender balance/nonce and recipient balance/nonce after this transaction. |

Receipts contain intermediate post-transfer values, which can differ from final account values after later transfers in the same block. Receipt presence means locally committed inclusion; absence means unknown to this store, not rejection or pending status. Mempool/RPC status belongs to later issues. No certificate or validator signing record is stored in this schema yet.

The commit API accepts only the next height with valid parent linkage, signatures and transaction/state commitments, or an exact byte-for-byte match of an already committed block at its original height. The latter returns `AlreadyApplied`, including for an older height. A different block at a committed height fails with `Conflict`; neither it nor an invalid new block changes state.

Any error after beginning the write path makes the handle unusable until closed and reopened. Reads also fail with `RecoveryRequired`, preventing a stale memory snapshot from being exposed after an uncertain commit. The consensus adapter must stop on *any* commit error; a failed commit must never be acknowledged based on an assumed rollback.

## Recovery and retained history

Opening the database replays every canonical block from the supplied genesis, checking signatures, parent linkage, heights and execution commitments. It compares every derived receipt, all final accounts (including nonces), the head and the exact row count with storage. Missing, extra or altered rows fail closed; unknown application versions are rejected without migration. Replay retains one block and account snapshots at a time, not a second copy of the entire history. Its startup cost grows with history and repeated full-state execution; bounded snapshots and pruning are future work.

`ledger()` returns the recovered snapshot. `block(height)` provides retained bytes for later synchronization, and `receipt(tx_id)` provides the durable inclusion record. History is retained without pruning, but this alone does not implement authenticated synchronization: #9/#11 must add certificate retention/verification and any necessary schema migration. Back up only while closed or through a future consistent backup interface; copying a live file is not supported.

Replay establishes application consistency, not historical finality or freedom from rollback. An internally consistent older copy of the database cannot be detected from application data alone. It must never authorize resumed validator signing without reconciling independent WAL/signing state.

## Malachite coordination required in #9

The pinned engine sends [`Decided` and `Finalized`](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/app-channel/src/msgs.rs); its [decision path](https://github.com/circlefin/malachite/blob/72143f6c99a98452b587e1c392bdb80944eb2232/code/crates/engine/src/consensus.rs) flushes its WAL before delivering a decision and waits for application acknowledgement before notifying synchronization of the committed height. The WAL and this database are separate durability domains, not a distributed transaction.

The integration must enforce this order:

1. Start with signing disabled. Validate the node identity/genesis binding, recover the application store, and reconcile Malachite WAL and durable signing state. Missing or stale signing information must fail closed; reconstructing balances cannot reconstruct safe signing history.
2. Bind the decision height and value ID to the exact canonical block. Require valid finality evidence for the genesis validator set (three distinct validators out of four equal weights), through the verified engine/adapter path. Validate synchronized certificates explicitly.
3. Call `commit_decided`. Only `Applied` or exact `AlreadyApplied` permits acknowledgement. On execution, conflict or storage error, stop; never skip the decision or blindly advance height.
4. On `Finalized`, do not execute payments again. Any richer certificate/evidence persistence is separate adapter work; it cannot debit balances again. Return the next height only after the application commit is known durable.
5. Before signing after restart, resolve WAL/application height differences and recover anti-equivocation state. Never reset WAL or keys merely to make heights agree.

| Crash/recovery point | Required integration behavior |
| --- | --- |
| Engine has a decision, application transaction did not commit | Replay the authenticated decision against the recovered parent and commit it. |
| Application committed, process stopped before acknowledgement | Redelivery returns `AlreadyApplied`; acknowledge without another debit. |
| Commit returned an error or process stopped during commit | Reopen and inspect the recovered history; replay the same authenticated decision. Do not infer whether it committed from the error alone. |
| Same height carries different block bytes | Halt as a safety/integrity fault. |
| Application is ahead of usable WAL/signing state, or either was restored from a stale backup | Stay unable to sign until validated reconciliation; application recovery alone is insufficient. |

The existing `ConsensusSigner` is still only a signing primitive. This issue does not claim end-to-end safe validator restart, an operational WAL adapter or verified network finality.

## Verification

```sh
cargo test -p rinpqc-node --locked storage::tests
just check
```

Tests use the independent signed-payment block fixture. Subprocesses exit without Rust destructors after writing a block, sender debit, recipient credit, receipt and head, immediately before commit, and after durable commit but before memory publication/acknowledgement. Reopening must expose either all old rows or all new rows, and replay must apply exactly once. These tests cover application transaction boundaries, not injected device errors inside redb's fsync implementation.

Additional tests cover multiple transfers to the same accounts, new accounts, empty blocks, historical duplicate decisions, invalid/conflicting blocks, corrupt history/balances/nonces/receipts/head/version, extra and missing rows, wrong genesis, exclusive file ownership, interrupted initialization and refusal to serve stale memory after interrupted publication. `just check` also runs the repository's formatting, build, lint, workspace tests, doctests and dependency policy gate.
