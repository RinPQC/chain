# Deterministic payment execution

The application now validates signed payments and ordered blocks against an immutable parent snapshot. This is the execution layer for M1; it does not start a network, persist state, establish finality, or expose payment RPC.

## API boundaries

`application::execution::Ledger::from_genesis` creates the initial account snapshot, chain identity, supply and consensus-bound block limits. Account rows are private and ordered by raw account ID. An absent account reads as zero balance and nonce; historical zero-balance accounts remain stored.

- `validate_transfer_bytes` checks the bounded canonical envelope, chain, keys/signature, then financial rules. It returns the proposed two-account update without changing the snapshot. It is useful for later local admission; it does not reserve a nonce or confirm settlement.
- `prepare_block` executes a caller-supplied ordered list on a separate candidate snapshot, produces transaction/state commitments and returns a `ValidatedBlock`.
- `validate_block` checks canonical size/count/header/parent/body commitment, decodes and validates each payment in block order, then checks the resulting state commitment. There is no alternative path for synchronized blocks to skip payment execution.
- `ValidatedBlock` exposes its block, candidate snapshot and ordered outcomes. These outcomes are speculative, not finalized RPC receipts. Dropping a candidate has no effect on its parent. The [application store](storage.md) supplies the durable parent and revalidates execution; the consensus adapter in #9 must authenticate finality before calling its commit API.

The executor follows the [specified rejection order](../specs/m1-payment-semantics.md). It uses checked subtraction/addition for balances and nonce increments. Supply is verified against the original genesis total after every candidate block. There are no fees, minting or burns.

Any failure rejects the complete block and returns the first failing transaction index and stable error code. An invalid later encoding does not mask an earlier financial failure. A duplicate or conflicting sender/nonce inside a block rejects the block; it is not silently skipped. A repeated valid submission against the same parent produces the same candidate, while replay against its resulting state is rejected. The application store implements durable repeated-decision idempotence and receipt lookup. RPC exposure remains separate work.

## Determinism and limits

State commitments use account-ID order, including each account's balance and next nonce. Transactions execute in the order supplied by the block. There is no wall-clock input, random ordering or dependence on local transaction arrival. A credit can be spent later in the same block.

The canonical header is 140 bytes. Blocks have a four-byte transaction count followed by 180-byte signed payments, giving `144 + 180 * count` bytes. Genesis limits are enforced before signature verification and candidate-state copying; malformed counts cannot drive an unbounded allocation. Header and body commitments follow the M1 domains and encodings.

The initial implementation clones the parent account map for each candidate and hashes the complete resulting state. This prioritizes isolated, understandable execution over throughput. It is not evidence of mobile suitability or a production performance claim; overlays or incremental commitments would need their own review.

## Verification

Run the execution tests with:

```sh
cargo test -p rinpqc-node --locked application::execution::tests
```

Tests consume all 16 published payment vectors using real Ed25519 signatures, including isolated defensive overflow states. They cover competing payments, replay, discarded candidates, zero-balance nonce retention, spending earlier credits, maximum supply/nonce boundaries, block limits and malformed commitments. `tests/fixtures/m1-payment-block.json` independently fixes the complete block bytes, block ID and resulting state root using OpenSSL signatures and Python struct/hashlib; it is checked against both construction and validation.

`just check` runs these tests alongside formatting, build, lint, other workspace tests, doctests and cargo-deny. The two existing live-DNS tests remain outside the offline suite.
