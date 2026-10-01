# Local M1 identities and genesis

The node now generates role-specific keys, derives canonical genesis/state commitments, validates local configuration, and initializes a bound data directory. The [consensus walkthrough](consensus.md) starts four validators and describes the current payment input. Linux/Unix key-file protections are implemented; other platforms fail closed for key storage.

## Try it locally

After installing the [build prerequisites](build.md), run this Bash example from the repository root. It creates fresh test keys outside the repository. Keep the directory if you want to reuse these identities.

```bash
cargo build --locked
node_bin="$PWD/target/debug/rinpqc-node"
identity_dir="$(mktemp -d)"
validators=()
for index in 1 2 3 4; do
  validators+=("$("$node_bin" keygen validator "$identity_dir/validator-$index.key")")
done
account="$("$node_bin" keygen transaction "$identity_dir/payment.key")"
"$node_bin" keygen network "$identity_dir/network.key"
"$node_bin" genesis-create "$identity_dir/genesis.json" "${validators[@]}" "$account"
chain_id="$("$node_bin" genesis-check "$identity_dir/genesis.json" | sed -n 's/^chain_id=//p')"
cat > "$identity_dir/node.toml" <<TOML
version = 1
genesis = "genesis.json"
expected_chain_id = "$chain_id"
validator_key = "validator-1.key"
network_key = "network.key"
data_dir = "node-data"
listen = "127.0.0.1:31001"
peers = []
TOML
"$node_bin" config-check "$identity_dir/node.toml"
"$node_bin" init "$identity_dir/node.toml"
printf 'Local identities: %s\n' "$identity_dir"
```

`genesis-create` sorts the four public validator keys, generates one network nonce from OS randomness, and allocates 4,000,000 test units to the supplied payment account. Validator weights are implicitly one. The complete manifest can instead be prepared manually: `genesis-check` rejects duplicate or unsorted entries rather than silently normalizing them. Its u64 fields are canonical decimal strings; public keys/nonces are lowercase hex.

Every node must receive the same trusted genesis and expected chain ID. Paths in node.toml are relative to the configuration file's resolved directory. For another validator, use its validator key, a separately generated network key, a distinct data directory and a different listen address. Optional `[[peers]]` entries contain `address = "127.0.0.1:31002"` and `public_key = "<network public key>"`; remove `peers = []` when using those entries. At most 64 peers are accepted. Peer keys and addresses must be unique and must not identify this node. Endpoint parsing is currently numeric IP and nonzero port only; the consensus adapter connects to these authenticated transport identities.

`config-check` makes no filesystem changes. `init` creates a private directory and binds it to the chain ID plus validator and network public keys. Repeating it with the same configuration is idempotent. An existing directory with a missing or mismatched marker fails closed, including an interrupted initialization; it is never silently adopted. This marker is not the signing WAL, a process lock, or a safe disaster-recovery protocol. The [consensus initialization and recovery files](consensus.md#durable-decisions-and-signing-readiness) supply those additional checks. A running node must not share its data directory with another process.

## Key storage and cryptographic boundaries

Key files use `RINKEY01 || suite:u16 || role:u8 || seed:bytes32` (43 bytes). Suite is 1; role codes are transaction=1, validator=2, network=3. Creation is exclusive, mode 0600, followed by file and directory sync. Existing files are never overwritten. Loading rejects final-component symlinks, nonregular files, extra/truncated data, wrong roles, multiple hard links and permissions other than 0600. Data directories are mode 0700. Use an owner-controlled parent directory; these checks do not defend against another process with the same account privileges or malicious replacement of ancestor directories. Secrets are unencrypted on disk; permissions provide the initial local protection, not a keystore or hardware-backed custody system.

The secret-key wrapper has no Debug/Clone implementation. Temporary seed buffers are zeroized and Dalek's signing-key zeroization is enabled. Keygen prints only the public key; parsing errors do not echo secret bytes. Do not use the public deterministic seeds in the test fixtures as node keys.

Ed25519 authorization uses `ed25519-dalek` 2.2.0 strict verification. Public keys must also have canonical compressed encoding, be in the prime-order subgroup, and not be low-order. Canonical transfer bytes and SHA-256 commitments follow [the M1 specification](../specs/m1-payment-semantics.md). Signature authorization does not imply sufficient funds or a valid account nonce; execution checks are implemented in the [payment executor](payments.md).

`TransactionSigner` and `ConsensusSigner` are separate interfaces with key-role enforcement. The latter is a cryptographic primitive only; it does not prevent double-signing and is not exposed as a CLI command. The running adapter uses its own [guarded, metadata-complete signatures](consensus.md#consensus-and-cryptographic-boundaries) instead. Its current signing input is:

`"RINPQC/CONSENSUS/v1\0" || version:u16 || suite:u16 || chain_id:bytes32 || kind:u8 || height:u64 || round:u32 || has_value:u8 || [value_id:bytes32]`.

Kind codes are proposal=1, prevote=2 and precommit=3. Heights start at one; rounds are 0 through i32::MAX. A proposal requires a value; votes may be nil (`has_value=0`, no value bytes). All integers are big-endian. This establishes distinct signing domains; the running engine format additionally authenticates proposal POL metadata and uses distinct domains. Replacing this primitive alone would not establish PQ network security.

## Evidence

`tests/fixtures/m1-identities.json` contains byte-exact signatures produced with OpenSSL 3.0.13 and genesis/state/transaction hashes independently assembled with Python struct/hashlib. Rust tests compare against these fixed results, check malformed encodings and weak keys, and exercise cross-chain/context rejection. Local configuration tests verify key permissions, role mismatch, peer duplication and directory binding. CLI tests exercise key generation and initialization without starting a network.
