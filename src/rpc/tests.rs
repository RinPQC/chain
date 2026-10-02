use super::*;
use crate::{
	application::Transfer,
	crypto::{KeyRole, SecretKey, TransactionSigner},
	genesis::Genesis,
};

fn fixture() -> (tempfile::TempDir, Store, SignedTransfer) {
	let dir = tempfile::tempdir().unwrap();
	let sender = SecretKey::from_seed(&[10; 32], KeyRole::Transaction);
	let recipient = SecretKey::from_seed(&[11; 32], KeyRole::Transaction);
	let mut validators =
		[1, 2, 3, 4].map(|n| SecretKey::from_seed(&[n; 32], KeyRole::Validator).public_key());
	validators.sort();
	let genesis = Genesis {
		network_nonce: [9; 32],
		max_block_bytes: 1_048_576,
		max_transactions: 4096,
		target_interval_ms: 5000,
		validators,
		accounts: vec![(sender.public_key(), 100)],
	};
	let chain = genesis.chain_id().unwrap();
	let payment = sender
		.sign_transfer(
			Transfer {
				chain_id: chain,
				sender: sender.public_key(),
				recipient: recipient.public_key(),
				amount: 30,
				nonce: 0,
			},
			&chain,
		)
		.unwrap();
	let store = Store::create(&dir.path().join("state.redb"), &genesis).unwrap();
	(dir, store, payment)
}
fn request(store: &Store, operation: Operation) -> Request {
	Request { version: 1, chain_id: hex::encode(store.ledger().unwrap().chain_id()), operation }
}
fn value(response: Response) -> Value {
	match response.outcome {
		Outcome::Result(value) => value,
		other => panic!("{other:?}"),
	}
}
fn error(response: Response) -> String {
	match response.outcome {
		Outcome::Error { code, .. } => code,
		other => panic!("{other:?}"),
	}
}
#[test]
fn pending_finalized_retries_and_forged_retries_are_distinct() {
	let (_dir, mut store, payment) = fixture();
	let mut queue = PaymentQueue::default();
	let status = || Operation::Transaction { tx_id: hex::encode(payment.transfer.id()) };
	assert_eq!(
		value(
			handle(
				request(&store, status()),
				&store,
				&mut queue,
				&crate::observability::Metrics::default()
			)
			.unwrap()
		)["status"],
		"unknown"
	);
	let submit = || Operation::Submit { signed_transfer: hex::encode(payment.encode()) };
	for _ in 0..2 {
		assert_eq!(
			value(
				handle(
					request(&store, submit()),
					&store,
					&mut queue,
					&crate::observability::Metrics::default()
				)
				.unwrap()
			)["status"],
			"pending"
		);
	}
	assert_eq!(store.ledger().unwrap().account(&payment.transfer.sender).balance, 100);
	assert_eq!(queue.len(), 1);
	let block = queue.propose(store.ledger().unwrap()).unwrap().block().encode().unwrap();
	store.commit_decided(1, &block).unwrap();
	queue.revalidate(store.ledger().unwrap());
	for _ in 0..2 {
		assert_eq!(
			value(
				handle(
					request(&store, submit()),
					&store,
					&mut queue,
					&crate::observability::Metrics::default()
				)
				.unwrap()
			)["status"],
			"finalized"
		);
	}
	assert_eq!(store.ledger().unwrap().account(&payment.transfer.sender).balance, 70);
	let mut forged = payment.clone();
	forged.signature[0] ^= 1;
	assert_eq!(
		error(
			handle(
				request(
					&store,
					Operation::Submit { signed_transfer: hex::encode(forged.encode()) }
				),
				&store,
				&mut queue,
				&crate::observability::Metrics::default(),
			)
			.unwrap()
		),
		"INVALID_SIGNATURE_OR_CHAIN"
	);
	assert_eq!(
		value(
			handle(
				request(&store, status()),
				&store,
				&mut queue,
				&crate::observability::Metrics::default()
			)
			.unwrap()
		)["height"],
		"1"
	);
}
#[test]
fn version_chain_and_encoding_errors_do_not_admit_payments() {
	let (_dir, store, _) = fixture();
	let mut queue = PaymentQueue::default();
	let mut req = request(&store, Operation::Status);
	req.version = 2;
	assert_eq!(
		error(handle(req, &store, &mut queue, &crate::observability::Metrics::default()).unwrap()),
		"UNSUPPORTED_VERSION"
	);
	let mut req = request(&store, Operation::Status);
	req.chain_id = hex::encode([0; 32]);
	assert_eq!(
		error(handle(req, &store, &mut queue, &crate::observability::Metrics::default()).unwrap()),
		"WRONG_CHAIN"
	);
	assert_eq!(
		error(
			handle(
				request(&store, Operation::Submit { signed_transfer: "00".repeat(181) }),
				&store,
				&mut queue,
				&crate::observability::Metrics::default(),
			)
			.unwrap()
		),
		"INVALID_ENCODING"
	);
	assert!(queue.is_empty());
}
#[tokio::test]
async fn malformed_oversized_and_slow_clients_are_bounded() {
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let address = listener.local_addr().unwrap();
	drop(listener);
	let (tx, _rx) = channel();
	let _server = Server::bind(address, tx).await.unwrap();
	for (bytes, expected) in
		[(b"not-json\n".to_vec(), "INVALID_REQUEST"), (vec![b'x'; MAX_FRAME + 1], "INVALID_FRAME")]
	{
		let mut stream = TcpStream::connect(address).await.unwrap();
		stream.write_all(&bytes).await.unwrap();
		let bytes =
			tokio::time::timeout(DEADLINE + Duration::from_secs(1), read_frame(&mut stream))
				.await
				.unwrap()
				.unwrap();
		assert_eq!(error(serde_json::from_slice(&bytes).unwrap()), expected);
	}
	let mut slow = TcpStream::connect(address).await.unwrap();
	slow.write_all(b"{").await.unwrap();
	let bytes = tokio::time::timeout(DEADLINE + Duration::from_secs(1), read_frame(&mut slow))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(error(serde_json::from_slice(&bytes).unwrap()), "TIMEOUT");
}

#[test]
fn pending_is_local_and_volatile_and_admission_errors_are_structured() {
	let (_dir, store, mut payment) = fixture();
	let mut queue = PaymentQueue::default();
	let sender = SecretKey::from_seed(&[10; 32], KeyRole::Transaction);
	payment.transfer.amount = 101;
	let payment =
		sender.sign_transfer(payment.transfer, &store.ledger().unwrap().chain_id()).unwrap();
	let response = handle(
		request(&store, Operation::Submit { signed_transfer: hex::encode(payment.encode()) }),
		&store,
		&mut queue,
		&crate::observability::Metrics::default(),
	)
	.unwrap();
	assert_eq!(error(response), "INSUFFICIENT_FUNDS");
	let mut transfer = payment.transfer;
	transfer.amount = 30;
	let payment = sender.sign_transfer(transfer, &store.ledger().unwrap().chain_id()).unwrap();
	handle(
		request(&store, Operation::Submit { signed_transfer: hex::encode(payment.encode()) }),
		&store,
		&mut queue,
		&crate::observability::Metrics::default(),
	)
	.unwrap();
	queue = PaymentQueue::default();
	let status = handle(
		request(&store, Operation::Transaction { tx_id: hex::encode(payment.transfer.id()) }),
		&store,
		&mut queue,
		&crate::observability::Metrics::default(),
	)
	.unwrap();
	assert_eq!(value(status)["status"], "unknown");
	assert_eq!(store.ledger().unwrap().height(), 0);
}

#[test]
fn metrics_are_bounded_and_distinguish_queue_from_committed_height() {
	let (_dir, store, payment) = fixture();
	let mut queue = PaymentQueue::default();
	queue.admit(store.ledger().unwrap(), &payment.encode()).unwrap();
	let metrics = crate::observability::Metrics::default();
	let response =
		handle(request(&store, Operation::Metrics), &store, &mut queue, &metrics).unwrap();
	assert!(serde_json::to_vec(&response).unwrap().len() < MAX_FRAME);
	let snapshot = value(response);
	assert_eq!(snapshot["queue_depth"], "1");
	assert_eq!(snapshot["committed_height"], "0");
	assert_eq!(snapshot["finalized_total"], "0");
}
