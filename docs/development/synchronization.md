# Verified history synchronization

An initialized M1 validator can now join late or return after downtime while its peers keep producing blocks. Start it with the same genesis, configuration, keys and private data directory used by the [local network walkthrough](consensus.md). The node automatically requests missing history through Malachite's default value-sync actor.

A fresh node in this four-validator PoC means a previously unused genesis validator identity with a newly initialized directory. It does not mean erasing the history of a validator that has already signed. Membership is still fixed; this change does not add dynamic admission or an observer-node CLI.

## Trust boundary

Peer-advertised tips are download hints, not trusted checkpoints or proof that a node has reached the latest global height. Starting from fixed genesis, each downloaded block must pass:

1. The chain-bound, versioned envelope and size/type checks.
2. A certificate with at least three distinct valid precommits from the four genesis validators, bound to the chain, height, round and block ID. Duplicate signers and vote extensions are not accepted as extra voting power.
3. Exact next-height and parent continuity, expected proposer context, canonical payment execution, transaction/state commitments and the certified block ID.

Malachite verifies the certificate before calling the host. The host independently verifies the certificate and full block execution as well. Because the pinned host callback omits the certificate, the application payload preserves it. The wire response encodes one certificate and reconstructs the engine's copy from that same record; inconsistent application/engine evidence cannot be encoded by the codec.

`SYNC_VERIFIED height=...` means the certified payload passed local validation and was cached. It is not an application commit. The ordinary `Decided` path verifies the certificate again, persists the certificate, commits the application store, updates the permitted signing height and acknowledges the decision. `COMMITTED` is printed only after durability. Only committed history is served to peers; cached downloads and unfinalized proposals are never advertised as finalized state.

## Bounds and retry behavior

M1 requests one height per batch with one outgoing request in flight. Status updates occur every second; requests time out after five seconds. Requests are limited to 128 bytes, responses to 2 MiB, individual blocks to both the absolute 1 MiB limit and their genesis limit, and certificates to four signatures. Malformed, multi-height and unavailable requests cannot make the host allocate or traverse an arbitrary history range.

Network envelope tags 11, 12 and 13 identify status, single-height request and optional single-block response. Tag 14 identifies the application payload carried through the host boundary. A response contains either no value or one block and its certificate. The sync protocol name includes the chain ID. Existing consensus signatures, proposal-part records and payment encodings are unchanged.

The selected Malachite sync actor handles peer scoring, request failures and retries. A [narrow discovery patch](../../vendor/README.md#malachite-discovery) preserves explicitly pinned peer identities across disconnects; persistent-only admission remains enabled. Peer-attributable decode, certificate and execution failures are rejected; no partial state is applied. A host callback that races a live commit is treated as a local sequencing condition, without penalizing its peer. Local storage errors stop the node rather than blaming a peer or acknowledging a failed write. Status claims cannot advance the ledger or unlock future signing heights. Unavailable peers may delay catch-up indefinitely; they do not authorize a shortcut around verification.

Downloads resume from the last durable application height. Incomplete network messages are discarded and requested again. A verified cached block without a persisted decision certificate is not replayed into state on startup. A persisted valid certificate with its payload can complete an interrupted application commit through the existing recovery path. No snapshot trust, pruning or state-only bootstrap is introduced: all blocks and certificates remain retained and are verified from genesis. Catch-up decisions use the upstream sync path, which bypasses the live block-cadence wait.

## Signing readiness and local recovery

Startup checks stored chain history, journal identity, finality evidence, WAL sequence, active signed-message correspondence and the signing high watermark before constructing an enabled signer. Missing or inconsistent files refuse startup; synchronization does not reset or manufacture signing history.

`SIGNING_READY next_height=...` means the local parent state and signing recovery checks passed. It does **not** claim that no newer block exists elsewhere. Every new proposal/vote signature is restricted to the height immediately following the locally verified durable head. The existing immutable slot guard still rejects conflicting or regressed signatures, and Malachite restores its active-height locks from the WAL. The readiness height advances only after a durable commit, never after a tip advertisement or download.

Immediate WAL replay remains enabled. A returning validator may participate at a historical next height while waiting for certified history; this is safe only under the preserved WAL/lock and signed-message history assumptions. There is no unauthenticated peer-tip threshold that can disable signing indefinitely.

Do not delete, replace or copy a used validator's recovery files to make it start. The existing fail-closed window between journaling a signature and recording its engine WAL entry remains. Total rollback, lost signing history or concurrent use of copied keys cannot be repaired safely by downloading blocks. Preserve the files and stop that identity pending a separately validated recovery procedure.

## Verification

`just check` covers bounded codec round trips and rejection, wrong-chain data, corrupt blocks, insufficient/duplicate/invalid signatures, invalid execution despite a quorum certificate, unavailable history, and the distinction between cached data, committed state and signing readiness. The subprocess suite covers late joining, interruption and resumption from a partial history, independent validator downtime, identical committed history and balances, preservation of prior signatures and renewed participation required for quorum while peers remain online.

These tests do not establish a catch-up throughput target or complete Byzantine network-load resistance. Full replay and execution costs, retained-history growth and upstream actor buffering remain resource limits to measure in later devnet acceptance work.
