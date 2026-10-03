# ADR 0002: M2 cryptographic profile

Status: accepted by owner-approved merge of [#48](https://github.com/RinPQC/chain/pull/48). Tracking: [#35](https://github.com/RinPQC/chain/issues/35), [M2 tracker](https://github.com/RinPQC/chain/issues/34). Date: 2026-10-03.

This proposal selects a concrete integration baseline. It does not activate PQ in the node. M1 remains classical until the dependent implementation tasks are merged. Acceptance of this ADR includes the dependency/distribution policy below; an unresolved objection keeps #35 open. No telemetry, explorer, ZK, permissionless membership, bridge or M1 history migration is included.

## Decision proposed

Use **Pure ML-DSA-87** for payment, validator and peer-identity signatures; use **ML-KEM-768** in the authenticated transport construction described below. Start a fresh M2 genesis with four equal-power validators and deterministic proposer rotation. Retain the pinned Malachite 0.8.0 engine revision `72143f6c99a98452b587e1c392bdb80944eb2232`.

Use **SHA-384 with 48-byte outputs** for application account identifiers, chain IDs, transaction/block IDs and state/transaction-list commitments. Do not truncate those outputs to retain M1 layouts. Use **SHA-512** in the proposed transport transcript/KDF profile and **ChaCha20-Poly1305** for records. The precise canonical byte encodings are implemented in #37; every commitment has a distinct, length-delimited domain and version. Cryptographic changes require a fresh test network, not reinterpretation of existing keys or history.

The signature implementation baseline is the published `qp-rusty-crystals-dilithium =4.1.1`, with defaults off and `ml-dsa-87` on. The comparison probe additionally enables `ml-dsa-65`; that is not permission to accept both in M2. The transport primitive baseline is published `clatter =1.1.0`, with defaults off and only `use-rust-crypto-ml-kem`, `use-chacha20poly1305`, `use-sha`. Its resolved KEM dependency is `ml-kem =0.2.3`. These versions identify a tested starting point, not an exemption from advisories or transport review.

Checksums, dependency features and license declarations are recorded in [the evidence inventory](../research/m2-profile-evidence.json), with a standalone [reproduction probe](../../experiments/m2-crypto-profile/README.md). The node's Cargo.toml, Cargo.lock and license policy remain unchanged in this PR.

## Alternatives and consequences

| Choice | Public key | Signature | Assessment |
| --- | ---: | ---: | --- |
| ML-DSA-65 | 1,952 bytes | 3,309 bytes | FIPS 204 category 3; viable smaller alternative, exercised by the probe |
| ML-DSA-87 | 2,592 bytes | 4,627 bytes | FIPS 204 category 5; proposed single signature profile, also matches the reviewed peer-identity candidate's family |

ML-DSA-87 adds 35.5% to the raw key-plus-signature subtotal compared with ML-DSA-65. Three individual validator signatures alone take 13,881 bytes before signer IDs or certificate framing. Neither aggregation nor hardware viability is assumed. #43 and #46 must set message/block limits and operating parameters from actual measurements; do not inherit a 4,096-payment block count blindly.

A single signature parameter set avoids algorithm-negotiation ambiguity across transaction, consensus and peer authorization. Its higher cost is acceptable as a PoC starting point, subject to measurements. Choosing ML-DSA-65 later would be an explicit version/profile change. Another FIPS-compatible implementation remains an alternative if licensing, compatibility or review disqualifies this one; do not silently substitute it behind the same unreviewed manifest.

ML-KEM-768 is category 3. A category-5 signature does not make the entire node category 5. Target approximately 128-bit resistance to generic quantum collision search for application commitments; a 384-bit ideal hash output gives that generic bound, whereas a 256-bit output gives approximately 85 bits. Quantum preimage search is a different property (approximately 192 versus 128 bits). These are idealized query-complexity comparisons, not concrete attack-cost guarantees or a complete protocol proof. SHA-384/512 avoids introducing a ZK-oriented permutation when no ZK is in scope.

## Signature, key and retry policy

- Use the FIPS 204 **pure** interface, with nonempty ASCII contexts no longer than 255 bytes. Do not use internal/raw signing functions, externally supplied `mu`, or HashML-DSA in this profile.
- Reserve distinct contexts: `RINPQC/M2/TRANSFER`, `RINPQC/M2/PROPOSAL`, `RINPQC/M2/PREVOTE`, `RINPQC/M2/PRECOMMIT`, `RINPQC/M2/VALIDATOR-PROOF`, `RINPQC/M2/PEER-AUTH`. Include the canonical chain ID and algorithm/profile version in the signed message. Include height, round, phase, value, signer, POL round and peer binding where relevant.
- Generate transaction, validator and network secrets independently from OS CSPRNG output. Seeds and fresh signing hedge inputs are 32 bytes. Entropy failure is fatal to that operation; never fall back to timestamps, fixed seeds or deterministic production signing. M2 does not require HD wallet support.
- Use **hedged signing** with fresh OS randomness for every new authorization. Deterministic mode is comparison/test-only. Both are standards-defined modes; neither proves implementation side-channel resistance.
- Consensus retry handling must look up the immutable slot and canonical unsigned message **before generating another signature**. Return the exact recorded signature on a matching retry, refuse conflicting bytes, and retain WAL-first release and authenticated-tail recovery. The current M1 signer re-signs before comparing encoded messages; that must change for hedged signatures in #39/#40.
- Distinct valid signatures for an identical payment must retain one logical transaction ID and one debit; every incoming envelope still undergoes authorization checks. Never deduplicate before verifying the supplied authorization.
- Keep full public keys separate from identifiers. Use an `Arc`/owned signer handle to satisfy the engine's cloneable private-key handle constraint, rather than cloning secret bytes. Bound imports, reject inconsistent key material, zeroize secrets and temporary buffers where supported, enforce restrictive file permissions and exclude secrets from Debug/logs/artifacts.

## Cryptographic coverage

| Path | M1 | Proposed M2 requirement | Task |
| --- | --- | --- | --- |
| Payments | Ed25519, fixed 180-byte transfer | ML-DSA-87; full key/account binding and canonical versioned envelope | #36–#38 |
| Proposals and both vote phases | Ed25519, chain-scoped bytes | ML-DSA-87 with separate contexts and unchanged quorum/locking | #39 |
| Finality certificates and history sync | Individual classical votes | Verify each PQ vote, signer uniqueness, membership and quorum; no aggregation assumption | #39, #40 |
| Validator proof | Classical signature over peer proof | PQ signature binds full network key/peer identity, validator key, chain and profile | #39, #41 |
| WAL and signing journal | Authenticated classical records | Exact PQ commitments; monotonic slots, locks and safe replay preserved | #40 |
| Account, chain, transaction and block IDs; state/body commitments | SHA-256 / shared 32-byte IDs | Domain-separated SHA-384, 48-byte application identifiers | #37, #38 |
| Peer ID | libp2p identifier | Routing handle only; authenticate and compare the full configured PQ public key independently | #41 |
| Identify and signed gossip metadata | libp2p identity signatures | PQ identity through every signed network record; no mandatory classical authorization bypass | #41 |
| Transport key establishment | TCP/Noise X25519 | ML-KEM-768 PQXX profile, authenticated as below | #42 |
| Record protection | Classical Noise configuration | ChaCha20-Poly1305 with session keys, monotonic nonces, replay/tamper rejection and bounded rekey/session lifetime | #42 |
| Transport transcript and KDF | Existing Noise stack | SHA-512 in the selected construction; fixed profile and chain-bound prologue | #42 |
| QUIC/TLS and alternate transports | Present in dependency graph | Disabled in M2 runtime unless independently meeting the same approved PQ obligations | #41, #42 |
| Keygen, signing hedge, KEM randomness, genesis nonce | OS randomness / library RNGs | Trace every RNG to OS CSPRNG; propagate entropy failures; review implicit library RNGs | #36, #42 |
| Leader selection | Deterministic round rotation | Unchanged; no VRF, beacon or random leader-selection claim | #35 |
| Storage checksums, gossip caches, routing hashes | Library-specific | Non-authoritative integrity/routing only; never substitute for signature or full-key verification | #40, #41 |
| RPC | Local/private PoC endpoint | PQ payment authorization; retain private deployment scope. Public RPC TLS/service authentication is not established by the peer transport | #44 |

The standard libp2p PeerId constructor hashes large public keys with SHA-256. Keep that wire-compatible handle initially, but never treat matching PeerId alone as proof of an approved identity: match the complete authenticated key against configured peers and verify validator binding. Collision-based routing disruption remains a limitation; no 128-bit quantum collision claim is made for PeerId. #41 must test a mismatched full key even when the supplied routing identifier is accepted. If the adapter cannot enforce that invariant, it is a blocker, not a reason to retain hash-only authorization.

## Transport integration plan

The candidate identity and Noise forks are reference implementations, **not drop-in selected dependencies**. Their inspected revisions are linked below. The identity manifest targets the older signature dependency `2.1.0`; the Noise manifest targets libp2p-core `0.42.0` and multihash `0.17.0`. Our locked stack uses libp2p `0.56.0`, core `0.43.2`, identity `0.2.14` and Noise `0.46.1`. A direct isolated build of the two forks fails dependency resolution on yanked `core2 0.4.0`, through multihash `0.17.0`. Do not force that dependency into the production graph.

Proposed route for #41/#42:

1. Keep the current libp2p core and Malachite APIs. Port the necessary PQ identity support onto the current identity package, using the selected signature API. Use a narrow, pinned, documented patch; test all identity/protobuf/signed-record consumers. Preserve required upstream notices.
2. Adapt the current TCP upgrade boundary to the proven primitive API rather than importing the old Noise dependency graph wholesale. The standalone primitive graph and a full-node dependency-resolution/compile probe coexist on Rust 1.93. This establishes feasibility at the dependency/primitive level, **not** a working authenticated Malachite network.
3. Use the library's PQXX pattern with ML-KEM-768 for static and ephemeral KEMs, ChaChaPoly records and SHA-512 transcript/KDF. This is a PQ Noise extension, not a FIPS-standardized complete authenticated protocol. Review the pattern, error paths and composition separately. The reference adapter uses SHA-256 and an unsafe extended RNG lifetime; do not copy either choice without review. Prefer scoped/owned safe RNG state in the new adapter.
4. Give the profile a distinct protocol ID and prologue carrying chain ID, profile and protocol versions. Authenticate the static KEM public key and peer identity in the handshake payload using the selected signature/context; verify that signature against the **actual remote static key returned by the handshake**, not an unrelated advertised key. Validate full-key allowlisting and validator proofs before allowing consensus traffic.
5. Negotiate only this profile. No classical fallback, no automatic profile selection from an untrusted peer, and no enabled QUIC/TLS bypass. Test cross-chain/prologue mismatches, substituted static keys, forged identities, malformed ciphertexts, replay, cancellation, handshake resource limits and downgrade attempts before accepting the transport.

A future hybrid construction is an alternative requiring its own fixed transcript/combiner specification. Do not invent a KEM combiner or claim that merely retaining an unused X25519 dependency creates a secure hybrid. The standalone probe does not establish peer authentication, transport forward secrecy, TCP framing, fragmentation, downgrade resistance or end-to-end quorum progress; those remain mandatory #41/#42/#45 evidence.

## Dependency and licensing disposition

The published signature archive declares **GPL-3.0** in both its package manifest and LICENSE; its registry checksum is verified. The repository landing page is not the controlling evidence for the selected archive. No alternative permissive grant has been established for that artifact.

The proposed policy is to accept this GPL-covered dependency for M2 and provide applicable GPLv3 notices and corresponding-source/build material when distributing a combined node binary or image. Original RinPQC MIT notices remain; an MIT package label alone must not be presented as the distribution terms of every linked component. #36 must add a package/version-scoped license exception and distribution notices after this policy is approved, not a blanket allowance or removal of license checking. If the owner does not accept these terms, select a verified permissively licensed implementation or obtain an applicable grant before runtime adoption. This ADR does not relicense third-party code or claim a legal audit.

The transport primitive is MIT; its resolved graph includes ML-KEM, ChaCha20-Poly1305, SHA-2, RNG and zeroization implementations with their own declarations. The selected signature crate pins `zeroize =1.8.2`, while M1 resolves `1.9.0`; the full-node probe resolves the shared dependency to 1.8.2. #36 must retain an explicit lockfile review and advisory check for this change rather than accepting an unexplained downgrade. No HD wallet, proof engine or Poseidon dependency is selected.

## Evidence and review limits

- The published signature archive identifies source revision `224c99a85237d5c7082a62022f750b684b453468`, **not** the previously inspected repository head `3b1464f751d536aba023a53df2a4f4533ab94f62`.
- The standalone Rust 1.93 release probe passes both signature variants, deterministic and hedged signing, wrong-context/message rejection, tampering and truncation. It also completes a four-message primitive PQXX handshake and rejects a tampered record. It prints no secrets.
- The package's six ACVP groups pass: for each of ML-DSA-65 and ML-DSA-87, 25 key-generation, 30 signature-generation and 15 signature-verification vectors (140 total). These are the package's vendored internal-interface vectors, not an independent certification or an external-interface interoperability audit. The archive omits two SHAKE fixture files referenced during compilation; reproduction restores those public files from the package's recorded source revision without modifying crypto source.
- The available software audit ended at `8bbe920dc5bfe88fa40028eb1ea9bae4e39a600c` on 2025-12-15. It covered the signature/HD-wallet source and excluded third-party dependencies and hardware attacks. It is **not** an audit of 4.1.1, the network forks, the transport composition or our node. Review the delta and domain-separation, entropy, zeroization and constant-time findings before runtime adoption.
- The new integration must retain all M1 correctness and recovery obligations. An unresolved mandatory identity/transport dependency blocks M2 completion even if payment signatures work. Benchmarks are tests; telemetry and explorer remain M3 or later.

## Sources

- [Published signature package](https://crates.io/crates/qp-rusty-crystals-dilithium/4.1.1), [release source and license](https://github.com/Quantus-Network/qp-rusty-crystals/tree/224c99a85237d5c7082a62022f750b684b453468/dilithium).
- [Identity reference](https://github.com/Quantus-Network/qp-libp2p-identity/tree/b9a7f46426efa2cf9b2ba20b95851ca18f361c95), [transport reference](https://github.com/Quantus-Network/qp-libp2p-noise/tree/901f09f30b32f910395270bba3a566191dc2f61f).
- [Signature software audit](https://github.com/Quantus-Network/qp-rusty-crystals/blob/3b1464f751d536aba023a53df2a4f4533ab94f62/audits/Neodyme-Audit.pdf).
- [Transport primitive package](https://crates.io/crates/clatter/1.1.0), [PQXX implementation](https://docs.rs/clatter/1.1.0/src/clatter/handshakestate/pq.rs.html).
- [FIPS 204](https://csrc.nist.gov/pubs/fips/204/final), [FIPS 203](https://csrc.nist.gov/pubs/fips/203/final). Review their published errata during integration.
- [GPLv3 distribution terms](https://www.gnu.org/licenses/gpl-3.0.html), sections 5–6.

- [NIST hash-security discussion](https://csrc.nist.gov/Projects/Post-Quantum-Cryptography/Post-Quantum-Cryptography-Standardization/Evaluation-Criteria/Security-%28Evaluation-Criteria%29): security categories and resource assumptions differ from simplified query bounds.
