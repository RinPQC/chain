# Running the M1 consensus node

The node now runs the pinned Malachite engine with four fixed, equal-power validators, classical Ed25519 signatures and authenticated TCP/libp2p connections. Validated payment blocks and empty blocks use the same proposal, vote, certificate and durable-commit path. There is no mempool, payment RPC or historical synchronization yet.

## First local network

Install the [build prerequisites](build.md), then run this Bash setup from the repository root. It generates fresh keys and a disposable genesis outside the repository. Ports 31001–31004 must be free.

```bash
cargo build --locked
node_bin="$PWD/target/debug/rinpqc-node"
run_dir="$(mktemp -d)"
validators=()
networks=()
for index in 0 1 2 3; do
  validators+=("$("$node_bin" keygen validator "$run_dir/validator-$index.key")")
  networks+=("$("$node_bin" keygen network "$run_dir/network-$index.key")")
done
account="$("$node_bin" keygen transaction "$run_dir/payment.key")"
"$node_bin" genesis-create "$run_dir/genesis.json" "${validators[@]}" "$account"
chain_id="$("$node_bin" genesis-check "$run_dir/genesis.json" | sed -n 's/^chain_id=//p')"
for index in 0 1 2 3; do
  cat > "$run_dir/node-$index.toml" <<TOML
version = 1
genesis = "genesis.json"
expected_chain_id = "$chain_id"
validator_key = "validator-$index.key"
network_key = "network-$index.key"
data_dir = "node-$index"
listen = "127.0.0.1:$((31001 + index))"
TOML
  for peer in 0 1 2 3; do
    if [ "$peer" != "$index" ]; then
      cat >> "$run_dir/node-$index.toml" <<TOML

[[peers]]
address = "127.0.0.1:$((31001 + peer))"
public_key = "${networks[$peer]}"
TOML
    fi
  done
  "$node_bin" init "$run_dir/node-$index.toml"
done
printf 'Network directory: %s\n' "$run_dir"
```

Start all four processes from the same shell:

```bash
node_pids=()
for index in 0 1 2 3; do
  "$node_bin" start "$run_dir/node-$index.toml" > "$run_dir/node-$index.log" 2>&1 &
  node_pids+=("$!")
done
tail -f "$run_dir"/node-*.log
```

Look for `ROUND` and `COMMITTED` lines. Every node should report the same `block_id` and `state_root` at a given committed height. Genesis initially targets five seconds per height, separately from round timeouts. Startup peer discovery is disabled; nodes connect only to the configured peer identities. Initial connection establishment or an unavailable proposer can require a later round before the first block.

Ctrl-C stops `tail`, not the background nodes. Stop those processes explicitly:

```bash
kill -INT "${node_pids[@]}"
wait "${node_pids[@]}"
```

For a restart, reuse the same files and run `start` again; do not rerun keygen or delete recovery data. Until #11, nodes cannot catch up on missed committed heights. Stop/restart the entire development network at a common observed committed height. If one node falls behind, preserve its data for diagnosis and use a fresh disposable network rather than resetting a validator's WAL or copying only its balances.

`RUST_LOG=info` enables upstream diagnostic logs. `COMMITTED` is emitted only after the application store confirms durability. These logs are the initial operational interface; stable metrics and payment RPC are later work.

## Payments in this stage

`start <config> [signed-payment-batch]` accepts an optional local fixture file containing concatenated canonical 180-byte signed transfers, up to 4096 transfers. It validates the batch against the recovered parent before starting. A proposer includes remaining fixture payments; already committed transaction IDs are omitted after restart. Supply the same valid fixture to the participating proposers for repeatable demonstrations. This input is not a queue, network submission endpoint, wallet, or replacement for #10/#12.

The four-process integration test generates fresh keys, signs a 30-unit payment from a 100-unit account, verifies the recipient credit and sender nonce, follows it with empty blocks, and restarts without a second debit:

```sh
cargo test -p rinpqc-node --locked --test consensus_network -- --nocapture
```

## Consensus and cryptographic boundaries

The adapter uses `EngineBuilder` with the default WAL, network and consensus actors. Membership comes from validated genesis. The proposer index is `(height - 1 + round) mod 4` in genesis validator order. The upstream quorum, locking, valid-round and timeout rules remain unchanged. Three distinct valid precommits are required; duplicate signatures cannot manufacture quorum. Default linear round timeouts start at three seconds for proposals and one second for prevote/precommit waits, growing by 500 ms per round with the upstream cap. Genesis `target_interval_ms` is passed separately as the height target, not as a finality deadline.

The engine uses proposal-and-parts mode: the vote value is the canonical block-header commitment, while one bounded part carries the complete block and an authenticated proposal descriptor. Before returning `Validity::Valid`, the host checks proposer identity, signature, height/round scope and full payment execution. Invalid or unauthenticated parts receive no proposed value and therefore no affirmative application verdict. At most four distinct valid parts are retained per height/round; admission is limited to the previous, current and immediately following round. This is a baseline resource bound, not a complete Byzantine load benchmark.

Consensus signatures use the following domains, followed by the 32-byte chain ID and the Borsh-encoded typed message:

- `RINPQC/MALACHITE/PROPOSAL/v1\0`: height, round, block value ID, POL round and proposer address.
- `RINPQC/MALACHITE/VOTE/v1\0`: height, round, optional value ID, validator address, vote type and absent extension.

Vote extensions are disabled. Proposal POL metadata is authenticated; the earlier generic `ConsensusSigner` primitive is not used by the running adapter because its preimage lacks that metadata. Validator proofs sign the pinned upstream `PoV` preimage binding validator public key and transport peer ID; this inner proof is intentionally network-agnostic. Transport keys remain separate from validator keys. All of these paths are classical M1 cryptography, not PQ security.

Network/WAL codecs wrap pinned Borsh representations in `RINPQC-NET || 0x00 || 0x01 || chain_id || type_tag`, with a 2 MiB envelope limit, exact consumption and no accepted trailing bytes. Tags 1–8 identify parts, signed consensus messages, liveness messages, streams, proposed values, polka certificates, commit certificates and validator proofs. Application block/transaction encodings remain unchanged. Changing these encodings or the upstream Borsh representations requires an explicit compatibility/version review.

Historical sync is disabled, and its required network codec methods reject every payload. #11 must implement authenticated catch-up; retaining blocks and certificates does not by itself provide it.

## Durable decisions and signing readiness

`init` creates `application.redb`, `consensus.redb`, `consensus.wal` and finally `consensus.ready` in the identity-bound private directory. Partial initialization fails closed. A ready directory with missing files is never repaired automatically. Opening the databases holds exclusive locks; a second process cannot share the same live store. Local directory ownership remains part of the threat model.

`consensus.redb` retains authenticated proposal payloads, commit certificates and signed messages with a high watermark over `(height, round, phase)`. Before releasing a signature, an immediate two-phase redb commit records the exact signed bytes. Repeating the identical slot/message is allowed; a conflicting message or a new regressed slot is refused. Write/signing-guard failures latch the journal closed. This journal supplements the engine WAL; it does not replace Malachite's lock recovery.

On a decision, the host independently verifies the certificate against genesis membership, verifies its link to the block, persists the certificate, commits the application transaction, and only then acknowledges `Decided`. `Finalized` advances height without executing payments again. A crash between the two database commits is reconciled by replaying the already certified block; a crash after the application commit is idempotent.

Startup verifies all retained certificates against the replayed application history, completes one interrupted certified application commit if present, and checks WAL sequence and active signed-message correspondence in both directions. Every locally journaled signature above the committed height must appear in the active WAL, and every active local WAL signature must appear in the journal. Missing, incompatible, corrupt or inconsistent data prevents signing.

There is an intentional fail-closed crash window: a signature may reach the durable journal before its matching entry reaches the engine WAL. If the process dies in that window, automatic restart is refused. Restoring availability for this case needs a separately validated recovery procedure; do not bypass the check. Likewise, simultaneous rollback of all local files or running copied keys on another machine cannot be detected by local checks alone. Device flush guarantees and private filesystem assumptions from the [storage contract](storage.md) still apply.

Payloads, certificates and signing history are retained without pruning, and recovery scans history. The adapter has two runtime worker threads and 8 MiB caches for each application/journal database; those numbers do not bound total memory or establish mobile suitability.

## Verification and remaining work

`just check` includes real subprocess/loopback tests for equal finalized state, payments exactly once, empty blocks, coordinated restart, replacement of a missing initial proposer, no finalization with two validators, and active-height WAL restart. Unit tests cover duplicate/conflicting/regressed signatures, chain/type/POL binding, invalid proposals, quorum/duplicate signers, interrupted certified commits and bounded codec rejection.

This completes the initial consensus integration boundary. The network is disposable and still lacks mempool admission (#10), historical catch-up (#11), payment RPC/CLI (#12), the full devnet tooling (#13) and the wider fault/acceptance suite (#14). Test coverage is evidence for these traces, not a proof of consensus correctness or a production-readiness claim.
