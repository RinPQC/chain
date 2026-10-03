# M2 canonical application profile

Status: implementation for review in #37, following accepted ADR 0002. The
`m2` module is the single M2 format boundary for the subsequent runtime adapters.
The running M1 adapters retain their explicitly classical formats until #38
(payments), #39 (consensus), #40 (history/WAL), and #41–#44 (network and tooling)
replace them. There is no mixed-profile runtime, format autodetection, migration
or claim that an M2 bootstrap directory already runs a validator.

## Identifiers and profile

Integers are unsigned, big endian. Application identifiers and commitments are
full 48-byte SHA-384 outputs, represented by distinct Rust types: `AccountId`,
`ValidatorId`, `ChainId`, `TransactionId`, `BlockId`, `StateRoot` and
`TransactionsRoot`. They are never public keys. Text IDs are exactly 96 lowercase
hex characters, without prefixes, whitespace or aliases; no address checksum or
human-friendly wallet address format is introduced yet. Account IDs are portable
across chains; replay protection comes from the signed transfer chain ID, not
from the address text. Wallet/RPC tooling must display and check the selected chain.

Each object starts with the nine-byte prefix:
`ASCII("RIN2") || u16(2) || u16(1) || u8(kind)`.
Version 2/profile 1 fixes Pure ML-DSA-87 (algorithm tag 87) and SHA-384 for the
application. It admits no alternative algorithms. The local PQ key-file version
remains independent of this wire version. Transport negotiation is specified in
#42, not by parsing an application header. Full public keys are exactly 2,592
bytes; signatures are exactly 4,627 bytes, using the approved library encodings.

All application hashes use this construction, with ASCII domain bytes `D`:

```text
H(D, P) = SHA-384(
  ASCII("RINPQC/M2/HASH") || u16(2) || u16(1) ||
  u16(len(D)) || D || u64(len(P)) || P
)
```

| Commitment | Domain | Payload |
| --- | --- | --- |
| Account ID | `ACCOUNT` | `u16(87) || full transaction public key` |
| Validator ID | `VALIDATOR` | `u16(87) || full validator public key` |
| Chain ID | `GENESIS` | Complete canonical genesis |
| Transaction ID | `TXID` | Complete unsigned transfer |
| Block ID | `BLOCK` | Complete header |
| Transaction list | `TXLIST` | `u32(count) || signed envelopes in block order` |
| State root | `STATE` | `u32(count) || (account ID, u64(balance), u64(next_nonce))*` |

State rows are strictly increasing by raw account-ID bytes, with no duplicates.
Zero-balance historical rows remain present. State hashing streams the rows and
retains the existing u32 account-count bound; genesis separately permits at most
65,536 allocations. The height-zero block anchor is an explicit `BlockId(chain_id)`
conversion. A block ID cannot be passed where an account ID is required.

## Canonical objects

Lengths below include the nine-byte prefix. All collections are bounded before
allocation. No implicit padding, optional fields, varints, ignored extension
bytes or JSON-to-binary coercions are permitted. Binary genesis is the canonical
configuration source for this boundary; user-facing JSON/CLI adapters remain #44.

| Kind | Object | Fields after prefix | Length |
| --- | --- | --- | ---: |
| 1 | Genesis | nonce[32], max_block_bytes:u32, max_transactions:u32, target_interval_ms:u64, four full validator keys, count:u32, (account ID, balance:u64)* | 10,429 + 56 × count |
| 2 | Unsigned transfer | chain ID, sender account ID, recipient account ID, amount:u64, nonce:u64 | 169 |
| 3 | Signed envelope | complete kind-2 transfer, full sender key, signature | 7,397 |
| 4 | Header | chain ID, height:u64, parent block ID, transactions root, state root | 209 |
| — | Block | complete kind-4 header, count:u32, complete kind-3 envelopes | 213 + 7,397 × count |
| 5 | Candidate finality certificate | chain ID, height:u64, round:u32, block ID, count:u8, (validator ID, signature)* | 118 + 4,675 × count |
| 7 | Local directory binding | chain ID, full local validator key, full network key | 5,241 |

Kind 6 is unassigned; it is not an accepted object. Future vote/proposal formats
must reserve explicit kinds and document their signature bytes in #39.

Genesis carries four unique validator keys sorted by their **validator IDs**.
Each retains equal voting power. Ordering by hashed IDs can change proposer order
relative to M1; this is a fresh genesis, not continuity of the old validator order. Allocations are sorted by account ID, unique,
positive and sum to at most u64::MAX. `Genesis::fresh` generates the network nonce
from fallible OS entropy; a new nonce changes the chain ID. Recipient accounts
need not disclose a public key before receiving funds. Spending requires a full
transaction-role public key whose derived account ID equals the sender. Unlike M1,
a recipient field is no longer a curve public key that can be validated on receipt.
Any canonical 48-byte account ID can receive funds; possession of a matching key
is established only when spending. This is the explicit address-validation change
required by hashed accounts; tooling must not imply proof of recipient ownership.

A transaction ID excludes the randomized signature and full-key envelope. The
unsigned sender ID binds that key through `ACCOUNT`; each incoming envelope must
still pass full-key binding and signature verification before admission or
idempotency checks. Sign the exact kind-2 bytes under `RINPQC/M2/TRANSFER` in #38.
The transaction-list commitment includes the full signed envelopes, so changing
an authorization changes the block commitment even when the logical payment ID
stays the same.

Certificates have three or four unique endorsements in increasing validator-ID
order. Height must be nonzero; round is bounded by i32::MAX for the engine.
Their IDs resolve to full keys in the pinned genesis. Decoding checks structural
canonicality only: #39 must verify membership, each corresponding precommit
signature, chain/height/round/value and quorum before treating one as finality.
A decoded certificate is not a verified certificate. The same distinction holds
for decoded payment envelopes and blocks: #38 supplies authorization and execution.

The existing resource ceilings remain 1 MiB per block, 4,096 transactions and
1–60,000 ms target interval; the byte ceiling also bounds the actual number of
larger M2 envelopes (at most 141 at 1 MiB). Genesis may select tighter limits,
including empty-block-only configurations as M1 allowed. These are parser ceilings,
not a throughput promise; #43/#46 choose operational limits after measurements.
No fees, balances, nonce rules, issuance or execution semantics change here.

## Rejection and fresh storage

Readers require the expected chain ID, exact object kind/profile, bounded counts
and exact byte lengths. Wrong-chain headers/transfers are rejected before public
key parsing. Genesis has no self-referential chain field: `decode_for_chain`
hashes its bounded bytes against an externally trusted expected ID before parsing
keys. An unpinned genesis decoder is for configuration inspection only.

`m2::storage::initialize` requires a previously nonexistent path in a trusted
parent directory. It creates a mode-0700 directory, writes/syncs mode-0600
`genesis.bin`, then writes/syncs `profile.bin` last and syncs the parent. The
binding contains the exact chain and local validator/network keys. The validator
must be a genesis member; the network key must differ from every validator key.

Existing paths, including empty directories, M1 directories and interrupted M2
initializations, are refused without deletion or adoption. `storage::check`
checks permissions, regular single-link files, no symlinks, exact genesis and
identity, and rejects unexpected files. This bootstrap format deliberately has
no database/WAL yet; #40 must introduce versioned runtime storage without allowing
M1 records to be reinterpreted. Other operating systems fail closed until an
equivalent filesystem permission model exists.

## Reproduction

`cargo test --locked --lib m2::tests` covers canonical roundtrips, profile/type/
chain confusion, key binding, altered counts, truncation/trailing bytes, limits,
certificate signer ordering and refusal to reuse or alter old directories.
[`scripts/m2-canonical-vectors.py`](../scripts/m2-canonical-vectors.py) independently
constructs the checked-in vectors using Python integer encoding and `hashlib`.
Its public keys come from the pinned NIST fixtures already in the repository.
Envelope/certificate signatures in these **structural** vectors are intentionally
invalid all-zero placeholders; tests explicitly show that payment verification
rejects them. Signature interoperability vectors remain in the PQ primitive tests.
