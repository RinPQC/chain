# Temporary network dependency compatibility patches

These patches remove the dependency findings tracked in [#20](https://github.com/RinPQC/chain/issues/20) while retaining the approved Malachite revision and libp2p 0.56 integration. They are committed source dependencies, not renamed registry releases. `[patch.crates-io]` makes their use explicit; their package versions satisfy upstream consumers' existing semver requirements.

## DNS adapter

`libp2p-dns/` derives from the published [libp2p-dns 0.45.0](https://crates.io/crates/libp2p-dns/0.45.0) source. Its upstream migration to Hickory 0.26 is retained. The manifest identifies the local compatibility package as 0.44.0 and binds libp2p-core 0.43 and libp2p-identity 0.2, matching the existing consumer. Hickory resolver/net/proto resolve to 0.26.3 in the active graph. The upstream MIT copyright and license text remain in the source files.

Two upstream tests require live external DNS and are explicitly ignored in the default offline test suite. Run them with `cargo test -p libp2p-dns --lib --features tokio --locked -- --ignored` on a network that permits DNS queries. Added offline regression tests cover TXT address parsing and construction through the transport API consumed by libp2p. These checks do not establish production networking readiness.

## Netlink packet core

`netlink-packet-core/` derives from the published [netlink-packet-core 0.8.2](https://crates.io/crates/netlink-packet-core/0.8.2) source. Its `paste` dependency aliases maintained `pastey` 0.2.3; the packet implementation is unchanged. The route example doctest imports are updated for the current route API. The unused example targets are omitted; the test-only route dependency uses 0.28 to match the current network stack. MIT license and README are retained. The upstream packet/macro tests run in the workspace.

## Maintenance

The sources are workspace members so their unit tests and doctests run in `just check`. They retain upstream formatting and lint policy; our formatting and Clippy gates apply to first-party Rust code. Cargo-deny still checks the complete resolved graph, including these packages, with no advisory ignores. Changes to patches require their tests and a full locked gate.

Remove both local overrides when an approved upstream Malachite/libp2p dependency graph supplies compatible fixes without these patches. Review source differences, restore registry dependencies, regenerate Cargo.lock and repeat the gate. Do not remove an override merely because a newer incompatible crate exists.
