# M2 signature primitives and key custody

`crypto::pq` implements Pure ML-DSA-87 behind the project crypto boundary.
The running node remains M1 until the transaction, consensus and networking
integration issues land. `SUITE = 87` is the local signature algorithm tag,
not a complete chain/transport profile or negotiated algorithm registry.

Generate each transaction, validator and network key independently with
`SecretKey::generate`. The API uses fallible OS entropy for both key generation
and every signing hedge. It exposes neither deterministic production signing
nor caller-selected contexts. Each `Domain` selects its fixed ADR 0002 FIPS
context and permitted key role. Canonical message encoding, chain/profile IDs,
expected signer binding and consensus authorization belong to the adapters in
#37–#41; the primitive API signs the supplied bytes and cannot establish those
application invariants. It is not a durable consensus signer.

Secret keys implement neither Clone, Debug nor serde serialization. Share an
`Arc<SecretKey>` if an engine handle needs Clone. The dependency wipes its secret
containers; seeds, hedges, exported buffers and file-read buffers also wipe on
drop. This does not guarantee removal of compiler-created copies, swap, crash
dumps or OS filesystem caches. Export is plaintext and intentionally explicit;
callers own and must wipe their import buffers. Role metadata guards accidental
misuse; it is not tamper-proof against an attacker who already owns the secret.

The local secret file is exactly `13 + ML-DSA-87 KEYPAIRBYTES` bytes:
`RINPQKEY` (8 bytes), file version `1` (u16 big endian), signature suite `87`
(u16 big endian), role (one byte: transaction=1, validator=2, network=3), then
the dependency's keypair encoding (secret followed by public). Import checks
exact length, version, suite, expected role and public/secret consistency.
This is a local key file format, not the M2 transaction or genesis wire format.
No M1 key migration is attempted.

Unix `files::save_new` creates mode-0600 files without replacing existing paths
and syncs the file and parent directory. `files::load` rejects symlinks, multiple
hard links, non-regular files, wrong permissions and incorrect size. Use trusted,
owner-controlled parent directories; ancestor symlink traversal and a malicious
same-user process are outside this file helper's protection. Other platforms
fail closed until an equivalent permission model is implemented.

`Signature::from_bytes` bounds size and rejects noncanonical hint encodings;
parsing alone never establishes validity. Always call `PublicKey::verify` to
check the signature, context and role. Restored public signature bytes can be
reused exactly: consensus adapters must look up persisted unsigned slot/message
bytes before calling `sign`, return the stored signature for a matching retry,
and reject conflicts. Two new signatures over the same message may differ.
No signature aggregation is introduced.

Tests cover OS entropy failure, role/context rejection, malformed imports,
key-file permissions, exact signature serialization, hedged variation and
[independent NIST external-interface vectors](../tests/fixtures/pq/README.md).
Run `cargo test --locked -p rinpqc-node crypto::pq::tests` or the full `just check`.
See [distribution obligations](distribution.md) before sharing binaries/images.
