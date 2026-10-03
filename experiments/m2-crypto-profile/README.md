# M2 primitive compatibility probe

This isolated workspace supports [ADR 0002](../../docs/decisions/0002-pq-cryptographic-profile.md). It does not modify the node's dependency graph or implement M2 consensus/networking. The probe source is MIT; the selected signature dependency is GPL-3.0. Review the ADR's distribution policy before incorporating it into the node or distributing a combined executable.

Run from the repository root:

```sh
cargo +1.93.0 run --release --locked --manifest-path experiments/m2-crypto-profile/Cargo.toml
cargo +1.93.0 clippy --locked --manifest-path experiments/m2-crypto-profile/Cargo.toml --all-targets -- -D warnings
```

The lockfile fixes all transitive versions and registry checksums. The inventory in `docs/research/m2-profile-evidence.json` records their license declarations and enabled features. `ct-internals`, wallet features, X25519 and PQClean Kyber are not enabled in this probe. ML-DSA-65 is enabled only for comparison; the proposed node profile accepts ML-DSA-87.

Expected output: one PASS line per signature variant, followed by a PASS for a four-message PQXX primitive handshake, record roundtrip and tamper rejection. The process uses OS randomness and prints only public sizes/results. It checks successful deterministic and hedged signatures, deterministic repeatability, context/message mismatch and malformed signatures. It does not benchmark performance.

The transport probe exercises ML-KEM-768, ChaCha20-Poly1305 and SHA-512 directly through the library. It does **not** authenticate configured peer identities, exercise TCP/libp2p framing, test downgrade resistance or prove protocol composition. Those are mandatory follow-up work, not consequences of this PASS output.

## Published signature test vectors

The 4.1.1 crate archive SHA-256 is `789877c169226a35d2ea686bbd9d506becc693f7bb0ee91acc03f74491e80c0f`. Its recorded source revision is `224c99a85237d5c7082a62022f750b684b453468`. Download the archive through crates.io, verify the checksum and extract into a temporary directory. Its test compilation references two omitted sibling files. Restore the following public fixtures from that exact revision into `test_vectors/` beside the extracted crate directory:

| Fixture | SHA-256 |
| --- | --- |
| `SHAKE256ShortMsg.rsp` | `ed3ea3fbc4e53d583a518499bc1a0156693b1b447adb6bfbad895443573976bd` |
| `SHAKE256LongMsg.rsp` | `8f8b67b0e5e11e7c690bca4bd55b31b62f59c8c985172f92b39896a6928648cd` |

Then run (substitute the extracted manifest path):

```sh
cargo +1.93.0 test --release --manifest-path <extracted-crate>/Cargo.toml \
  --lib --no-default-features --features ml-dsa-65,ml-dsa-87 acvp -- --nocapture
```

Observed: six groups passed, 140 vectors total. This executes the package's vendored internal-interface vectors; it is not independent certification. Do not modify cryptographic source to make tests pass. The crate archive's dev-test graph is separate from this probe's locked runtime dependency graph.

[Fixture source](https://github.com/Quantus-Network/qp-rusty-crystals/tree/224c99a85237d5c7082a62022f750b684b453468/test_vectors).

## Integration evidence boundaries

A separate temporary package depending on the current node, signature 4.1.1 (`ml-dsa-87`) and this minimal transport primitive graph resolved and passed `cargo +1.93.0 check --locked`. It inherited the repository's three existing patches and started from the M1 lockfile. The baseline was commit `83aa5a5` and the graph change is recorded in the inventory. This proves dependency/compiler coexistence only; it did not replace any node algorithm.

Directly combining the inspected identity/Noise forks failed on the yanked `core2 0.4.0` dependency. No production dependency or license exemption was changed to suppress that failure. ADR 0002 instead proposes a narrow port onto the current libp2p interfaces.
