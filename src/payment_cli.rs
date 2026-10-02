//! Offline signing and the bounded local RPC client; private keys never enter requests.
use crate::{
	application::{SignedTransfer, Transfer, SIGNED_TRANSFER_LEN},
	crypto::{KeyRole, TransactionSigner},
	genesis::{parse_id, Genesis, MAX_GENESIS_JSON},
	infrastructure::{load_key, read_bounded, write_new},
	rpc::{Operation, Outcome, Request, Response},
};
use std::path::Path;

pub fn run(args: &[String]) -> eyre::Result<bool> {
	let request = match args {
		[cmd, key, genesis, recipient, amount, nonce, output] if cmd == "payment-sign" => {
			let genesis = Genesis::from_json(&read_bounded(Path::new(genesis), MAX_GENESIS_JSON)?)?;
			let key = load_key(Path::new(key), KeyRole::Transaction)?;
			let chain = genesis.chain_id()?;
			let amount = amount.parse::<u64>()?;
			let nonce = nonce.parse::<u64>()?;
			let recipient = parse_id(recipient)?;
			eyre::ensure!(
				amount > 0 && recipient != key.public_key() && nonce < u64::MAX,
				"invalid amount, recipient or exhausted nonce"
			);
			crate::crypto::public_key(&recipient)?;
			let signed = key.sign_transfer(
				Transfer { chain_id: chain, sender: key.public_key(), recipient, amount, nonce },
				&chain,
			)?;
			write_new(Path::new(output), &signed.encode())?;
			println!(
				"{}",
				serde_json::json!({"tx_id": hex::encode(signed.transfer.id()), "chain_id": hex::encode(chain)})
			);
			return Ok(true);
		},
		[cmd, address, file] if cmd == "payment-submit" => {
			let signed =
				SignedTransfer::decode(&read_bounded(Path::new(file), SIGNED_TRANSFER_LEN)?)?;
			crate::crypto::verify_transfer(&signed, &signed.transfer.chain_id)?;
			(
				address,
				Request {
					version: 1,
					chain_id: hex::encode(signed.transfer.chain_id),
					operation: Operation::Submit { signed_transfer: hex::encode(signed.encode()) },
				},
			)
		},
		[cmd, address, chain] if cmd == "chain-status" || cmd == "metrics" => {
			parse_id(chain)?;
			(
				address,
				Request {
					version: 1,
					chain_id: chain.clone(),
					operation: if cmd == "metrics" {
						Operation::Metrics
					} else {
						Operation::Status
					},
				},
			)
		},
		[cmd, address, chain, id] if cmd == "account" || cmd == "transaction" => {
			parse_id(chain)?;
			parse_id(id)?;
			let operation = if cmd == "account" {
				Operation::Account { account: id.clone() }
			} else {
				Operation::Transaction { tx_id: id.clone() }
			};
			(address, Request { version: 1, chain_id: chain.clone(), operation })
		},
		_ => return Ok(false),
	};
	let address = request.0.parse()?;
	let result = tokio::runtime::Builder::new_current_thread()
		.enable_all()
		.build()?
		.block_on(crate::rpc::call(address, &request.1));
	let response = result.unwrap_or_else(|_| Response::error("RPC_UNAVAILABLE", "Check the local RPC address and node; outcome may be unknown, query status or retry the identical signed request."));
	println!("{}", serde_json::to_string(&response)?);
	eyre::ensure!(
		matches!(response.outcome, Outcome::Result(_)),
		"RPC request failed; see the structured response"
	);
	Ok(true)
}
