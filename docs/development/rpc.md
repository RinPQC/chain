# Local payment RPC and CLI

M1 provides an opt-in, loopback-only payment interface. It reads the node's locally verified committed history and admits signed payments into the existing bounded queue. Keys are created and used for signing locally; the server has no signing or key-upload operation. This is a development API, not an authenticated public gateway or a light-client proof service.

## Run a payment

Use the [four-validator walkthrough](consensus.md) from the same Bash shell. It enables RPC on ports 32001–32004 and funds `payment.key` with 4,000,000 native test units. Keep the node processes running after exiting `tail` with Ctrl-C. The variables below come from that walkthrough.

```bash
rpc_address="127.0.0.1:32001"
recipient="$("$node_bin" keygen transaction "$run_dir/recipient.key")"
"$node_bin" chain-status "$rpc_address" "$chain_id"
"$node_bin" account "$rpc_address" "$chain_id" "$account"
"$node_bin" payment-sign "$run_dir/payment.key" "$run_dir/genesis.json" "$recipient" 30 0 "$run_dir/payment.bin"
"$node_bin" payment-submit "$rpc_address" "$run_dir/payment.bin"
```

The fresh funded account has nonce `0`. For later payments, read `next_nonce` from the account response and sign a new file; amounts and nonces are decimal integers in native test units. Signing is offline: it checks encoding, key role, chain identity and basic payment fields, but cannot promise sufficient current balance or an available nonce. Existing output files are never overwritten.

`payment-sign` prints the stable `tx_id` and `chain_id`. Copy its transaction ID to query the result:

```bash
# Replace this value with the tx_id printed by payment-sign.
tx_id="<transaction-id>"
"$node_bin" transaction "$rpc_address" "$chain_id" "$tx_id"
"$node_bin" account "$rpc_address" "$chain_id" "$account"
"$node_bin" account "$rpc_address" "$chain_id" "$recipient"
```

Poll transaction status until `finalized` before treating the payment as settled. The sender then has balance `3999970` and next nonce `1`; the new recipient has balance `30`. Submission may initially return `pending`. Repeating the identical command is safe:

```bash
"$node_bin" payment-submit "$rpc_address" "$run_dir/payment.bin"
```

A finalized retry returns the original inclusion height and block ID without enqueuing or debiting again, including after a normal restart. An invalid signature is rejected even if the unsigned transaction ID already has a receipt. On a timeout or lost response, the outcome may be unknown: query that ID or resend the identical file. Do not create a replacement payment with a new nonce merely because an RPC response was lost.

Successful CLI RPC commands print one JSON response and exit zero. Server errors and transport unavailability print a structured error response and exit nonzero. Invalid local arguments, key files or signed files fail before connection, with a bounded local error on stderr. `keygen` continues to print only the public account ID. The server never receives the private key path or key material.

## Enable the endpoint

Add a top-level field before any `[[peers]]` tables:

```toml
rpc_listen = "127.0.0.1:32001"
```

Omitting the field disables RPC. Each node needs a different port, separate from its consensus port. Non-loopback and zero-port configurations are rejected. Bind failure prevents startup; an unexpected listener failure stops the node. Stop and restart with the same recovery files when changing the address. IPv6 loopback is supported using a socket address such as `[::1]:32001`.

Run the CLI on the node's host, or explicitly forward a loopback port over SSH. RPC has no authentication or TLS: local users able to connect can read state and submit already-signed payments. Do not expose this endpoint as a public service. The [Compose devnet](devnet.md) accesses RPC through commands executed inside each container. Public gateway policy remains outside M1.

## Version 1 wire format

The transport is TCP, **not HTTP or JSON-RPC 2.0**. A connection carries one compact UTF-8 JSON request terminated by a newline and one JSON response terminated by a newline, then closes. There is no batching, subscription or persistent session. Both CLI and server use the same schema. Unknown request fields and methods are rejected.

Every request includes `version: 1`, the expected genesis-derived `chain_id`, and an `operation` object:

```json
{"version":1,"chain_id":"<64 lowercase hex characters>","operation":{"method":"status"}}
```

| Method | Additional operation fields | Result |
| --- | --- | --- |
| `metrics` | None | Per-process diagnostic counters and current queue/height observations; see the [devnet guide](devnet.md#metrics-and-logs) |
| `status` | None | `chain_id`, committed `height`, `block_id`, `state_root` |
| `account` | `account`: account ID | `account`, committed `balance`, `next_nonce`, observation `height` |
| `transaction` | `tx_id`: transaction ID | `tx_id`, `status`; finalized results also include `height`, `block_id` |
| `submit` | `signed_transfer`: 360 hex characters encoding one canonical 180-byte signed transfer | Same result shape as `transaction` |

IDs are lowercase hex. Returned heights, balances and nonces are decimal strings, preserving the full unsigned 64-bit range in clients with limited numeric precision. `status` describes the local durable head; it does not claim that this node has reached the newest height elsewhere. Account responses are snapshots at their returned height; separate calls are not an atomic multi-account snapshot.

Response examples:

```json
{"version":1,"result":{"tx_id":"<id>","status":"pending"}}
{"version":1,"result":{"tx_id":"<id>","status":"finalized","height":"12","block_id":"<id>"}}
{"version":1,"error":{"code":"WRONG_CHAIN","message":"Use the chain ID from the trusted genesis."}}
```

- `unknown`: no durable receipt and no matching entry in this node's current queue. It does not prove global non-inclusion or rejection.
- `pending`: admitted to this node's volatile queue after signature and payment checks. It does not reserve funds, promise propagation/inclusion, or establish finality. A restart can lose pending entries, and committed competing state can remove them.
- `finalized`: a durable receipt exists in locally verified history. Consensus or verified synchronization must pass the certificate and execution checks before the host commits that state. RPC does not return an independently verifiable certificate; clients trust the selected local node.

Errors use stable `code` and explanatory `message` fields. Schema/transport errors include `INVALID_REQUEST`, `INVALID_FRAME`, `UNSUPPORTED_VERSION`, `WRONG_CHAIN`, `INVALID_ACCOUNT`, `INVALID_TX_ID`, `INVALID_ENCODING`, `INVALID_SIGNATURE_OR_CHAIN`, `UNAVAILABLE`, `TIMEOUT` and `RATE_LIMITED`. Admission exposes existing payment/queue codes such as `INSUFFICIENT_FUNDS`, `NONCE_TOO_LOW`, `NONCE_TOO_HIGH`, `NONCE_CONFLICT` and `QUEUE_FULL`. The CLI maps unavailable or invalid transport responses to `RPC_UNAVAILABLE`. A connection failure can occur before any response exists.

## Bounds and isolation

Requests and client response reads are capped at 2,048 bytes excluding the newline. The server allows eight active connections and eight queued host calls. Reading and processing have a combined three-second deadline; writing has a separate three-second deadline. The CLI has a four-second overall timeout. Slow or disconnected clients do not retain unbounded tasks. The host processes at most 16 RPC operations per one-second window, including reads; excess calls return `RATE_LIMITED`. Kernel connection backlogs can cause excess clients to time out rather than receive a structured error.

The host serializes RPC with queue admission and durable block decisions. It never opens a second application database or serves a separate state cache. Expired calls not yet started are skipped. Failure of local storage propagates to stop the host, rather than acknowledge stale or unpersisted state. Pending-ID lookup is bounded by the queue's 4,096 entries; no unbounded status-history cache is created. These bounds are development defaults, not a throughput or denial-of-service guarantee.

## Verification

`just check` includes real four-process CLI payment submission, finality and balance queries, retry before/after finalization and after a normal node restart, plus unavailable-service errors. Unit tests cover forged finalized retries, version/chain mismatches, malformed/oversized/slow frames, local volatile pending status and admission rejection. Configuration tests reject public RPC binds. Existing consensus, execution, storage, synchronization and signing-recovery tests remain part of the same gate.
