# M1 payment, genesis and block semantics

Status: proposed M1 development-network specification. Tracking: [#4](https://github.com/RinPQC/chain/issues/4). These choices have no mainnet economic significance. Review this document before implementation; changes to accepted scope remain separate decisions.

## Profile and canonical encoding

M1 uses one native test coin, four fixed validators of voting power one, Ed25519 authorization, and SHA-256 commitments. These are concrete classical-profile proposals for the disposable M1 network, not PQ claims. M2 starts with new genesis and selects its own cryptographic profile.

All integers below are unsigned, fixed-width, big-endian: `u16`, `u32`, and `u64` mean 2, 4, and 8 bytes. `bytes32` means exactly 32 bytes. Concatenation has no implicit padding, separators or length fields. Strings used as domain tags are literal ASCII bytes including the final NUL (`\0`). Variable arrays have an explicit count followed by exactly that many fixed-format records. Reject unknown versions, truncated inputs, trailing bytes, inconsistent counts, and noncanonical encodings. JSON is an RPC presentation, never the signed or hashed representation. RPC represents all u64 values as decimal strings without signs, spaces, or leading zeroes except `"0"`.

Version is `u16(1)`. Cryptographic suite is `u16(1)` for this M1 profile. An account ID is the raw 32-byte Ed25519 public key. RPC displays IDs and hashes as 64 lowercase hexadecimal characters without a prefix. No address checksum or alternative aliases are introduced. Authorization must use strict Ed25519 verification, rejecting malformed keys/signatures and weak-key cases; #6 must select and test a library/configuration that provides this behavior. Algorithm identifiers are mandatory even though M1 admits only one suite. Transaction, validator and transport keys have separate roles; tooling generates separate keys by default.

## Genesis and network identity

Canonical genesis `G` is this ordered sequence:

1. `version:u16`, `suite:u16`, `network_nonce:bytes32`.
2. `max_block_bytes:u32`, `max_transactions:u32`, `target_interval_ms:u64`.
3. Exactly four validator public keys (`bytes32` each), strictly ascending lexicographic byte order, unique and valid for the selected verifier. All weights are implicitly one.
4. `account_count:u32`, followed by accounts strictly ascending by account ID, each `account_id:bytes32 || balance:u64`.

Reject duplicate accounts/validators, invalid account keys, zero genesis balances, an empty allocation, more than 65,536 allocated accounts, or total supply exceeding `u64::MAX`. Genesis nonces are all zero. Compute totals in checked wider arithmetic. The manifest is trusted configuration supplied out of band, not something an unknown peer may select. Nodes compare the expected chain ID before synchronization or signing.

`chain_id = SHA256("RINPQC/GENESIS/v1\0" || G)`. The network nonce distinguishes deliberately separate networks with otherwise identical configuration. Generate it once when creating the manifest, never independently on each node. All changes to G produce a different network identity and require fresh M1 state.

Proposed devnet defaults are 1,048,576 maximum block bytes, 4,096 maximum transactions, and a 5,000 ms target interval. The canonical block size includes its header, count and signed transactions; it excludes the finality certificate, which has its own bounded encoding. Require `144 <= max_block_bytes <= 1,048,576`, `max_transactions <= 4,096`, and `1 <= target_interval_ms <= 60,000`. Empty blocks are valid. These conservative limits are initial test settings, not measured capacity. Round timeout configuration is separate from the target and does not influence state validity.

An example development allocation assigns four separately generated accounts 1,000,000 base units each (total 4,000,000). No decimal denomination, rewards, minting, burns or fees exist in M1. Genesis balances are test fixtures, not contributor allocations. Supply remains equal to the genesis sum at every committed height.

## Transfers and identifiers

The unsigned transfer is exactly 116 bytes:

`version:u16 || suite:u16 || chain_id:bytes32 || sender:bytes32 || recipient:bytes32 || amount:u64 || nonce:u64`.

Signing input is `"RINPQC/TRANSFER/v1\0" || unsigned_transfer`. The 64-byte Ed25519 signature follows the unsigned transfer; a signed transfer is exactly 180 bytes. `tx_id = SHA256("RINPQC/TXID/v1\0" || unsigned_transfer)`. Excluding the signature means that alternate authorization encodings cannot create multiple logical IDs for the same instruction; every submitted envelope must nevertheless pass strict signature verification before admission or duplicate classification. Equality of tx IDs does not exempt invalid bytes from validation.

Accounts are represented by `(balance, next_nonce)`. An absent account reads as `(0,0)` and is created by a successful credit. Keep existing zero-balance accounts and their nonces; never reset a sender's replay state when its balance reaches zero. Values range from zero to `u64::MAX`. Transfer amount must be positive and sender must differ from recipient. A successful transfer requires `nonce == sender.next_nonce`, deducts amount, credits recipient and increments the sender's next nonce exactly once. A sender with next nonce `u64::MAX` cannot transfer because the increment would overflow. Receiving remains allowed.

Validate each transaction in the order below; stop on the first failure. Checks use the current speculative state after preceding transactions in the same block:

| Order | Check | Error |
| --- | --- | --- |
| 1 | Exact canonical envelope, supported version and suite | `INVALID_ENCODING` / `UNSUPPORTED_VERSION` / `UNSUPPORTED_SUITE` |
| 2 | Chain ID matches configured genesis | `WRONG_CHAIN` |
| 3 | Sender and recipient key encodings valid; signature strictly verifies over the exact signing input | `INVALID_KEY` / `INVALID_SIGNATURE` |
| 4 | Amount is nonzero | `ZERO_AMOUNT` |
| 5 | Sender and recipient differ | `SELF_TRANSFER` |
| 6 | Next nonce is incrementable | `NONCE_EXHAUSTED` |
| 7 | Nonce equals next nonce | `NONCE_TOO_LOW` / `NONCE_TOO_HIGH` |
| 8 | Sender balance covers amount | `INSUFFICIENT_FUNDS` |
| 9 | Recipient credit fits u64 | `BALANCE_OVERFLOW` |

Within check 1, parse length first, then version, then suite. Within check 3, validate sender key, recipient key, then signature. Failure changes no state. Recipient overflow is unreachable from a valid supply-bounded genesis under these transitions but remains a defensive check and isolated unit-test case. A duplicate transaction inside a proposed block fails nonce validation on its second occurrence and invalidates the entire block. Do not skip invalid entries or partially commit the block.

## Ordering and local submission

The proposer chooses transaction order; that ordered list is committed in the block. Validators execute precisely that order, never local arrival order, map iteration order or wall-clock order. Accounts may spend credits received earlier in the same block. Conflicting transfers with identical sender/nonce can each be valid against the parent state, but only the first applicable one can succeed; including both invalidates the block.

The initial mempool admits only transactions whose nonce equals the committed next nonce and that pass committed-state validation. Keep at most one pending instruction per sender/nonce: an exact valid resubmission returns the existing ID; a different ID returns `NONCE_CONFLICT`. There is no fee replacement policy. Future-nonce submissions are rejected and may be retried after earlier finalization. These are local admission rules, not extra block-validity rules. Revalidate the pool after every finalized block and after restart. Nodes may have different pending pools and still agree on finalized state.

Rate limits, queue exhaustion and transport failures are local RPC errors, not consensus invalidity. #10 specifies bounded eviction and proposal construction. A proposer may include a valid transaction it did not previously admit to its local pool.

## State, blocks and commitments

Canonical state `S` is `account_count:u32` followed by all stored accounts in strictly increasing ID order, each `account_id:bytes32 || balance:u64 || next_nonce:u64`. Include zero-balance historical accounts. Reject a state transition that would make the account count exceed `u32::MAX`; report `STATE_CAPACITY` after the balance checks above. `state_root = SHA256("RINPQC/STATE/v1\0" || S)`. This whole-state commitment is intentionally simple; it is not a Merkle proof service and may become a scalability limit.

Genesis is height zero with `block_id = chain_id` and the genesis-derived state root. The first produced block has height one. A block header is exactly 140 bytes:

`version:u16 || suite:u16 || chain_id:bytes32 || height:u64 || parent_block_id:bytes32 || transactions_root:bytes32 || state_root:bytes32`.

A canonical body is `transaction_count:u32 || signed_transfer[transaction_count]`; `transactions_root = SHA256("RINPQC/TXLIST/v1\0" || body)`. A block is `header || body`, with a minimum size of **144 bytes** and size `144 + 180 * transaction_count`.

`block_id = SHA256("RINPQC/BLOCK/v1\0" || header)` is the consensus value ID. Validate size/count before expensive signature verification; validate header version/suite/chain, height exactly parent height plus one without overflow, known parent ID, body commitment, all ordered transfers, then resulting state commitment. No timestamp or proposer ID is included: proposer eligibility and signatures belong to authenticated consensus messages, while the same value can be re-proposed in a later round. Repeated IDs must resolve to identical canonical block bytes.

Validator ordering is the genesis ordering. For height `h >= 1` and nonnegative round `r`, proposer index is `((h - 1) mod 4 + r mod 4) mod 4`. Reject invalid/sentinel rounds before conversion. Weight and quorum rules remain Malachite's strict greater-than-two-thirds rule: three distinct validator signatures are required. #9 must bind engine proposer and vote signing bytes to chain ID, version, message kind, height, round, voting phase, and value/nil identity without changing engine locking rules.

Store a versioned finality envelope separately from the block ID; it binds chain ID, height, round and block ID to individual validator signatures. Its concrete engine-compatible encoding and nil-vote representation are #9 integration work, not a new payment format. At most four unique validator entries are accepted, checked against genesis; duplicates cannot contribute voting power. Unknown signers, wrong context or invalid signatures invalidate the evidence. A certificate is necessary for sync finality but never substitutes for block execution. No aggregate-signature or compact-proof claim is made.

## Persistence, recovery and RPC outcomes

On a valid decision, atomically persist the block, height/head ID, resulting balances and nonces, finalized transaction IDs with height/index, and the corresponding verified finality evidence. A repeated decision for the already committed same block is idempotent; a conflicting block at a committed height is a safety error. Do not acknowledge `Decided` until the application commit is durable. `Finalized` may later enrich stored evidence; this does not re-execute payments. No successful payment depends on the end of the target-interval timer.

Signing WAL and application storage need coordinated recovery but are not assumed to share one database transaction. #8/#9 must define recovery checkpoints and failure injection before enabling signing. Missing, stale or inconsistent signing metadata fails closed; synchronization alone does not authorize clearing that metadata or signing again. Publish a coherent committed snapshot to readers, including after crashes.

| RPC result/status | Meaning |
| --- | --- |
| `pending` | Validated for this node's local pool; no settlement promise. May disappear after eviction or restart. |
| `finalized` | Valid certificate and executed block durably committed locally; return block ID, height and transaction index. |
| `unknown` | No local pending or finalized record; does not prove global rejection or absence. |
| Submission rejection | Return error code and ID when safely computable; does not create a permanent on-chain failed transaction. |

After verifying the submitted envelope, a transaction already finalized returns its existing result rather than `NONCE_TOO_LOW`. A changed instruction with an already consumed nonce is rejected. Every block inclusion still follows nonce checks and rejects duplicate execution. Invalidly signed resubmissions never obtain admission through the duplicate shortcut. Balance, nonce and status queries identify their committed snapshot height; pending effects are excluded. RPC timeouts have unknown outcome: clients resubmit the same signed transaction or query its ID, never assume that timeout means rollback.

## Deterministic vectors and implementation gates

[Execution vectors](m1-payment-vectors.json) define account A/B/C state and ordered operations, expected state or the first rejection. Account letters are aliases for distinct valid test keys; `signature_valid` models the authorization predicate, not an invented signature fixture. Cases beginning from impossible supply or nonce boundary states are explicitly defensive unit tests. Block cases roll back every speculative change on any failure.

[Encoding vector](m1-encoding-vector.json) supplies exact unsigned bytes, signing input and transaction ID for a synthetic encoding fixture (no signature-validity claim). All integer values in the execution-vector JSON are decimal strings.

#6 adds byte-exact Ed25519 known-answer vectors using a pinned verifier, including invalid keys, signatures and cross-domain signatures. #7 consumes these execution vectors; #8 tests atomic crash boundaries; #9/#11 test certificates and synchronization. This document and its examples have not demonstrated any running implementation. Cryptographic evidence and engine serialization remain explicit downstream gates.

## Additional rejection and submission vectors

These mutations are applied independently to an otherwise valid fixture. Earlier checks must pass unless the row explicitly tests precedence. They are normative test expectations; implementation tests will instantiate cryptographic keys in #6.

| Fixture / mutation | Expected result |
| --- | --- |
| Transfer envelope length 179 or 181 instead of 180 | `INVALID_ENCODING`; no effects |
| Correct-length envelope with version 2 | `UNSUPPORTED_VERSION` |
| Version 1 and suite 2 | `UNSUPPORTED_SUITE` |
| Chain ID differs by one bit; signature otherwise verifies for those bytes | `WRONG_CHAIN` |
| Sender or recipient rejected by the strict key validator | `INVALID_KEY` |
| Valid keys but signed for another domain or altered amount | `INVALID_SIGNATURE` |
| Zero amount and self-transfer simultaneously | `ZERO_AMOUNT` (precedence) |
| New recipient with already `u32::MAX` stored accounts | `STATE_CAPACITY` (synthetic boundary fixture) |
| Valid resubmission of a finalized transfer | Existing finalized height/index; no new effects |
| Invalid signature attached to a finalized transfer's unsigned bytes | `INVALID_SIGNATURE`; no duplicate shortcut |
| Valid exact resubmission while pending | Existing pending ID; one pool entry |
| Different valid pending instruction for the same sender/nonce | `NONCE_CONFLICT`; retain existing entry |
| Genesis duplicate or unsorted accounts/validators | Reject genesis before startup |
| Genesis balances 1 and `u64::MAX` | Reject genesis supply overflow |
| Genesis account with zero balance | Reject genesis noncanonical allocation |
| Empty block after genesis | Height 1, parent ID equals chain ID, unchanged state/supply |
| Block height 2 immediately after genesis or wrong parent | Reject block; retain genesis state |
| Body count exceeds configured maximum or length disagrees with count | Reject block before execution |
| Block size 1 byte above configured maximum | Reject block before execution |
| Correct body but wrong transactions root | Reject block before execution |
| Correct payments but wrong resulting state root | Reject whole block; no effects |
| Valid certificate paired with invalid payments | Reject sync value; do not advance |
| Two-vote certificate, duplicate signer, unknown signer or wrong signed context | Reject finality evidence |

Genesis, block and certificate rejection categories above do not yet prescribe public RPC error names: those inputs are not payment-submission requests. No invalid block is partially accepted. Supply is checked against the genesis total after every successful vector, including empty blocks and repeated finalized submissions.
