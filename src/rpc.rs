//! Bounded local payment RPC. Only the consensus host accesses committed state and the queue.
use crate::{
	application::SignedTransfer, genesis::parse_id, mempool::PaymentQueue, storage::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::SocketAddr, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::{TcpListener, TcpStream},
	sync::{mpsc, oneshot},
	task::{JoinHandle, JoinSet},
};

pub const MAX_FRAME: usize = 2048;
const DEADLINE: Duration = Duration::from_secs(3);
const CAPACITY: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
	pub version: u16,
	pub chain_id: String,
	pub operation: Operation,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
	Status,
	Metrics,
	Account { account: String },
	Transaction { tx_id: String },
	Submit { signed_transfer: String },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
	pub version: u16,
	#[serde(flatten)]
	pub outcome: Outcome,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Result(Value),
	Error { code: String, message: String },
}
impl Response {
	fn ok(value: Value) -> Self {
		Self { version: 1, outcome: Outcome::Result(value) }
	}
	pub fn error(code: &str, message: &str) -> Self {
		Self { version: 1, outcome: Outcome::Error { code: code.into(), message: message.into() } }
	}
}

pub struct Call {
	pub request: Request,
	pub reply: oneshot::Sender<Response>,
}
/// Dropping the host aborts the listener and its bounded set of connections.
pub struct Server(JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}
impl Server {
	pub async fn bind(address: SocketAddr, calls: mpsc::Sender<Call>) -> std::io::Result<Self> {
		if !address.ip().is_loopback() || address.port() == 0 {
			return Err(std::io::Error::other("RPC requires a nonzero loopback address"));
		}
		let listener = TcpListener::bind(address).await?;
		Ok(Self(tokio::spawn(async move {
			let mut tasks = JoinSet::new();
			loop {
				tokio::select! {
					_ = calls.closed() => break,
					_ = tasks.join_next(), if !tasks.is_empty() => {},
					connection = listener.accept(), if tasks.len() < CAPACITY => {
						let Ok((stream, _)) = connection else { break; };
						let calls = calls.clone();
						tasks.spawn(serve_connection(stream, calls));
					}
				}
			}
		})))
	}
	pub fn is_finished(&self) -> bool {
		self.0.is_finished()
	}
}
pub fn channel() -> (mpsc::Sender<Call>, mpsc::Receiver<Call>) {
	mpsc::channel(CAPACITY)
}

async fn read_frame(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
	let mut bytes = Vec::with_capacity(512);
	loop {
		let byte = stream.read_u8().await?;
		if byte == b'\n' {
			return Ok(bytes);
		}
		if bytes.len() == MAX_FRAME {
			return Err(std::io::Error::other("frame exceeds 2048 bytes"));
		}
		bytes.push(byte);
	}
}
async fn serve_connection(mut stream: TcpStream, calls: mpsc::Sender<Call>) {
	let response = match tokio::time::timeout(DEADLINE, async {
		let bytes = match read_frame(&mut stream).await {
			Ok(bytes) => bytes,
			Err(_) => {
				return Response::error(
					"INVALID_FRAME",
					"Send one JSON line of at most 2048 bytes.",
				)
			},
		};
		let request = match serde_json::from_slice(&bytes) {
			Ok(request) => request,
			Err(_) => {
				return Response::error("INVALID_REQUEST", "Check the version 1 request schema.")
			},
		};
		let (reply, result) = oneshot::channel();
		if calls.try_send(Call { request, reply }).is_err() {
			return Response::error(
				"UNAVAILABLE",
				"Service is busy or stopping; retry the identical request.",
			);
		}
		result.await.unwrap_or_else(|_| {
			Response::error(
				"UNAVAILABLE",
				"Service stopped; query status or retry the identical signed request.",
			)
		})
	})
	.await
	{
		Ok(response) => response,
		Err(_) => Response::error(
			"TIMEOUT",
			"Outcome may be unknown; query status or retry the identical signed request.",
		),
	};
	if let Ok(mut bytes) = serde_json::to_vec(&response) {
		bytes.push(b'\n');
		let _ = tokio::time::timeout(DEADLINE, stream.write_all(&bytes)).await;
	}
}

/// The caller serializes this with durable decisions; no second database owner or state cache.
/// Storage failures propagate to stop the host instead of presenting stale state.
pub fn handle(
	request: Request,
	store: &Store,
	pending: &mut PaymentQueue,
	metrics: &crate::observability::Metrics,
) -> eyre::Result<Response> {
	if request.version != 1 {
		return Ok(Response::error("UNSUPPORTED_VERSION", "Use RPC version 1."));
	}
	let ledger = store.ledger()?;
	if parse_id(&request.chain_id).ok() != Some(ledger.chain_id()) {
		return Ok(Response::error("WRONG_CHAIN", "Use the chain ID from the trusted genesis."));
	}
	let result = match request.operation {
		Operation::Metrics => metrics.snapshot(ledger.height(), pending.len()),
		Operation::Status => {
			json!({"chain_id": hex::encode(ledger.chain_id()), "height": ledger.height().to_string(), "block_id": hex::encode(ledger.block_id()), "state_root": hex::encode(ledger.state_root())})
		},
		Operation::Account { account } => {
			let Ok(key) = parse_id(&account) else {
				return Ok(Response::error(
					"INVALID_ACCOUNT",
					"Expected a lowercase 32-byte hex account ID.",
				));
			};
			let state = ledger.account(&key);
			json!({"account": account, "balance": state.balance.to_string(), "next_nonce": state.next_nonce.to_string(), "height": ledger.height().to_string()})
		},
		Operation::Transaction { tx_id } => {
			let Ok(id) = parse_id(&tx_id) else {
				return Ok(Response::error(
					"INVALID_TX_ID",
					"Expected a lowercase 32-byte hex transaction ID.",
				));
			};
			transaction(store, pending, id)?
		},
		Operation::Submit { signed_transfer } => {
			if signed_transfer.len() != 2 * crate::application::SIGNED_TRANSFER_LEN {
				return Ok(Response::error(
					"INVALID_ENCODING",
					"Expected one 180-byte signed transfer encoded as hex.",
				));
			}
			let signed = hex::decode(&signed_transfer)
				.ok()
				.and_then(|raw| SignedTransfer::decode(&raw).ok());
			let Some(signed) = signed else {
				return Ok(Response::error(
					"INVALID_ENCODING",
					"Invalid signed transfer encoding.",
				));
			};
			// IDs exclude signatures. Even a finalized retry must authenticate its signed bytes.
			if crate::crypto::verify_transfer(&signed, &ledger.chain_id()).is_err() {
				return Ok(Response::error(
					"INVALID_SIGNATURE_OR_CHAIN",
					"Verify the signature and chain ID locally.",
				));
			}
			let id = signed.transfer.id();
			if store.receipt(&id)?.is_none() {
				if let Err(error) = pending.admit(ledger, &signed.encode()) {
					return Ok(Response::error(
						&error.to_string(),
						"Payment was not admitted; check balance, nonce and queue capacity.",
					));
				}
			}
			transaction(store, pending, id)?
		},
	};
	Ok(Response::ok(result))
}
fn transaction(
	store: &Store,
	pending: &PaymentQueue,
	id: crate::crypto::Id,
) -> eyre::Result<Value> {
	Ok(if let Some(receipt) = store.receipt(&id)? {
		json!({"tx_id": hex::encode(id), "status": "finalized", "height": receipt.outcome.height.to_string(), "block_id": hex::encode(receipt.block_id)})
	} else {
		json!({"tx_id": hex::encode(id), "status": if pending.contains(&id) { "pending" } else { "unknown" }})
	})
}

/// Bounded CLI transport. A lost response never authorizes constructing a new payment.
pub async fn call(address: SocketAddr, request: &Request) -> eyre::Result<Response> {
	eyre::ensure!(
		address.ip().is_loopback() && address.port() != 0,
		"RPC requires a nonzero loopback address"
	);
	let mut bytes = serde_json::to_vec(request)?;
	eyre::ensure!(bytes.len() <= MAX_FRAME, "request exceeds RPC frame limit");
	bytes.push(b'\n');
	let response = tokio::time::timeout(DEADLINE + Duration::from_secs(1), async {
		let mut stream = TcpStream::connect(address).await?;
		stream.write_all(&bytes).await?;
		let bytes = read_frame(&mut stream).await?;
		let response: Response = serde_json::from_slice(&bytes)?;
		eyre::ensure!(response.version == 1, "unsupported RPC response version");
		Ok::<_, eyre::Report>(response)
	})
	.await
	.map_err(|_| {
		eyre::eyre!("RPC timeout; query status or retry the identical signed request")
	})??;
	Ok(response)
}

#[cfg(test)]
mod tests;
