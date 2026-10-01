use super::*;
use crate::{
	application::Transfer,
	crypto::{KeyRole, SecretKey, TransactionSigner},
};
use serde::Deserialize;

fn fixture_genesis() -> Genesis {
	let f: serde_json::Value =
		serde_json::from_str(include_str!("../../../tests/fixtures/m1-identities.json")).unwrap();
	Genesis::from_json(&serde_json::to_vec(&f["genesis"]).unwrap()).unwrap()
}
fn key(alias: &str) -> SecretKey {
	SecretKey::from_seed(
		&[match alias {
			"A" => 1,
			"B" => 2,
			"C" => 3,
			_ => panic!("unknown test account"),
		}; 32],
		KeyRole::Transaction,
	)
}
fn signed(chain_id: Id, sender: &str, recipient: &str, amount: u64, nonce: u64) -> SignedTransfer {
	key(sender)
		.sign_transfer(
			Transfer {
				chain_id,
				sender: key(sender).public_key(),
				recipient: key(recipient).public_key(),
				amount,
				nonce,
			},
			&chain_id,
		)
		.unwrap()
}
#[derive(Deserialize)]
struct Amounts {
	balance: String,
	next_nonce: String,
}
#[derive(Deserialize)]
struct Operation {
	sender: String,
	recipient: String,
	amount: String,
	nonce: String,
	signature_valid: bool,
}
#[derive(Deserialize)]
struct Vector {
	name: String,
	initial: BTreeMap<String, Amounts>,
	transfers: Vec<Operation>,
	expected_error: Option<String>,
	expected_state: BTreeMap<String, Amounts>,
}
#[derive(Deserialize)]
struct Vectors {
	cases: Vec<Vector>,
}
fn accounts(rows: BTreeMap<String, Amounts>) -> BTreeMap<Id, Account> {
	rows.into_iter()
		.map(|(alias, a)| {
			(
				key(&alias).public_key(),
				Account {
					balance: a.balance.parse().unwrap(),
					next_nonce: a.next_nonce.parse().unwrap(),
				},
			)
		})
		.collect()
}
#[test]
fn all_specification_vectors_with_real_authorization() {
	let vectors: Vectors =
		serde_json::from_str(include_str!("../../../docs/specs/m1-payment-vectors.json")).unwrap();
	assert_eq!(vectors.cases.len(), 16);
	for v in vectors.cases {
		let mut parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
		// Test-only construction includes intentionally unreachable overflow fixtures.
		parent.accounts = accounts(v.initial);
		parent.supply = parent
			.accounts
			.values()
			.try_fold(0u64, |n, a| n.checked_add(a.balance))
			.unwrap_or(u64::MAX);
		let before = parent.clone();
		let before_root = parent.state_root();
		let txs = v
			.transfers
			.iter()
			.map(|op| {
				let mut tx = signed(
					parent.chain_id,
					&op.sender,
					&op.recipient,
					op.amount.parse().unwrap(),
					op.nonce.parse().unwrap(),
				);
				if !op.signature_valid {
					tx.signature[0] ^= 1;
				}
				tx
			})
			.collect();
		let result = parent.prepare_block(txs);
		match v.expected_error {
			Some(expected) => {
				let ExecutionError::Payment { reason, .. } = result.unwrap_err() else {
					panic!("wrong rejection for {}", v.name)
				};
				assert_eq!(reason.to_string(), expected, "{}", v.name);
				assert_eq!(parent.accounts, accounts(v.expected_state));
			},
			None => {
				let result = result.unwrap();
				assert_eq!(result.next.accounts, accounts(v.expected_state), "{}", v.name);
				assert_eq!(result.next.supply, parent.supply);
			},
		}
		assert_eq!(parent, before, "{} mutated the parent", v.name);
		assert_eq!(parent.state_root(), before_root);
	}
}
#[test]
fn prepare_validate_and_repeat_produce_identical_results() {
	let g = fixture_genesis();
	let parent = Ledger::from_genesis(&g).unwrap();
	assert_eq!(parent.state_root(), g.state_root().unwrap());
	let txs =
		vec![signed(parent.chain_id, "A", "B", 30, 0), signed(parent.chain_id, "B", "C", 50, 0)];
	let candidate = parent.prepare_block(txs.clone()).unwrap();
	assert_eq!(candidate, parent.prepare_block(txs).unwrap());
	assert_eq!(candidate, parent.validate_block(&candidate.block.encode().unwrap()).unwrap());
	assert_eq!(
		candidate.next.account(&key("A").public_key()),
		Account { balance: 70, next_nonce: 1 }
	);
	assert_eq!(
		candidate.next.account(&key("B").public_key()),
		Account { balance: 0, next_nonce: 1 }
	);
	assert_eq!(
		candidate.next.account(&key("C").public_key()),
		Account { balance: 50, next_nonce: 0 }
	);
	assert_eq!(candidate.outcomes[1].index, 1);
	assert_eq!(candidate.outcomes[1].height, 1);
	assert_eq!(candidate.outcomes[0].effect.tx_id, candidate.block.transfers[0].transfer.id());
	assert_eq!(candidate.next.accounts.values().map(|a| a.balance).sum::<u64>(), 120);
	assert_eq!(parent.height(), 0);
	assert_eq!(parent.account(&key("A").public_key()).balance, 100);
	// Dropping one candidate cannot affect a competing candidate from the same parent.
	drop(candidate);
	let alternative =
		parent.prepare_block(vec![signed(parent.chain_id, "A", "C", 100, 0)]).unwrap();
	assert_eq!(alternative.next.account(&key("C").public_key()).balance, 100);
}
#[test]
fn empty_blocks_replay_and_retained_zero_balance_nonce() {
	let parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
	let empty = parent.prepare_block(vec![]).unwrap();
	assert_eq!(empty.block.encode().unwrap().len(), MIN_BLOCK_LEN);
	assert_eq!(empty.next.state_root(), parent.state_root());
	assert!(empty.outcomes.is_empty());
	let transfer = signed(parent.chain_id, "A", "B", 100, 0);
	let first = parent.prepare_block(vec![transfer.clone()]).unwrap();
	assert_eq!(first.next.account(&key("A").public_key()), Account { balance: 0, next_nonce: 1 });
	assert!(first.next.accounts.contains_key(&key("A").public_key()));
	assert_eq!(first.next.validate_transfer(&transfer), Err(PaymentError::NonceTooLow));
	assert_eq!(
		first.next.validate_block(&first.block.encode().unwrap()),
		Err(ExecutionError::WrongHeight)
	);
	let second = first.next.prepare_block(vec![signed(parent.chain_id, "B", "A", 10, 0)]).unwrap();
	assert_eq!(second.next.account(&key("A").public_key()), Account { balance: 10, next_nonce: 1 });
	assert!(second.next.prepare_block(vec![signed(parent.chain_id, "A", "C", 10, 1)]).is_ok());
}
#[test]
fn competing_and_duplicate_transfers_reject_whole_block() {
	let parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
	let before = parent.clone();
	let a = signed(parent.chain_id, "A", "B", 80, 0);
	let b = signed(parent.chain_id, "A", "C", 70, 0);
	assert!(parent.prepare_block(vec![a.clone()]).is_ok());
	assert!(parent.prepare_block(vec![b.clone()]).is_ok());
	for txs in [vec![a.clone(), b.clone()], vec![b, a.clone()], vec![a.clone(), a]] {
		assert_eq!(
			parent.prepare_block(txs),
			Err(ExecutionError::Payment { index: 1, reason: PaymentError::NonceTooLow })
		);
		assert_eq!(parent, before);
	}
}
#[test]
fn payment_error_precedence_and_defensive_boundaries() {
	let mut parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
	assert_eq!(
		parent.validate_transfer(&signed(parent.chain_id, "A", "A", 0, 99)),
		Err(PaymentError::ZeroAmount)
	);
	let mut bad = signed(parent.chain_id, "A", "A", 0, 99);
	bad.signature = [0; 64];
	assert_eq!(parent.validate_transfer(&bad), Err(PaymentError::InvalidSignature));
	bad.transfer.chain_id = [0; 32];
	assert_eq!(parent.validate_transfer(&bad), Err(PaymentError::WrongChain));
	let mut bad = signed(parent.chain_id, "A", "B", 1, 0);
	bad.transfer.recipient = [0; 32];
	assert_eq!(parent.validate_transfer(&bad), Err(PaymentError::InvalidKey));
	let valid = signed(parent.chain_id, "A", "B", 1, 0).encode();
	for len in [0, 179, 181] {
		assert_eq!(
			parent.validate_transfer_bytes(&vec![0; len]),
			Err(PaymentError::InvalidEncoding)
		);
	}
	for (offset, reason) in
		[(1, PaymentError::UnsupportedVersion), (3, PaymentError::UnsupportedSuite)]
	{
		let mut bytes = valid.clone();
		bytes[offset] = 2;
		assert_eq!(parent.validate_transfer_bytes(&bytes), Err(reason));
	}
	parent.accounts.get_mut(&key("A").public_key()).unwrap().next_nonce = u64::MAX;
	assert_eq!(
		parent.validate_transfer(&signed(parent.chain_id, "A", "B", 1, 0)),
		Err(PaymentError::NonceExhausted)
	);
	assert_eq!(check_capacity(u32::MAX as usize, false), Err(PaymentError::StateCapacity));
	assert!(check_capacity(u32::MAX as usize, true).is_ok());
	parent.height = u64::MAX;
	assert_eq!(parent.prepare_block(vec![]), Err(ExecutionError::WrongHeight));
}
#[test]
fn bounded_block_decoding_and_commitments() {
	let mut g = fixture_genesis();
	g.max_block_bytes = (MIN_BLOCK_LEN + SIGNED_TRANSFER_LEN) as u32;
	g.max_transactions = 1;
	let parent = Ledger::from_genesis(&g).unwrap();
	let before = parent.clone();
	let tx = signed(parent.chain_id, "A", "B", 1, 0);
	let proposal = parent.prepare_block(vec![tx.clone()]).unwrap();
	let bytes = proposal.block.encode().unwrap();
	assert_eq!(parent.validate_block(&bytes).unwrap(), proposal);
	assert_eq!(parent.prepare_block(vec![tx.clone(), tx]), Err(ExecutionError::BlockLimits));
	for n in 0..bytes.len() {
		assert!(parent.validate_block(&bytes[..n]).is_err());
	}
	let mut bad = bytes.clone();
	bad.push(0);
	assert_eq!(parent.validate_block(&bad), Err(ExecutionError::BlockLimits));
	for (offset, expected) in [
		(1, ExecutionError::UnsupportedProfile),
		(4, ExecutionError::WrongChain),
		(43, ExecutionError::WrongHeight),
		(44, ExecutionError::WrongParent),
		(76, ExecutionError::TransactionsRoot),
		(108, ExecutionError::StateRoot),
	] {
		let mut bad = bytes.clone();
		bad[offset] ^= 1;
		assert_eq!(parent.validate_block(&bad), Err(expected));
	}
	let mut bad = bytes.clone();
	bad[HEADER_LEN..MIN_BLOCK_LEN].copy_from_slice(&u32::MAX.to_be_bytes());
	assert_eq!(parent.validate_block(&bad), Err(ExecutionError::BlockLimits));
	let mut bad = bytes.clone();
	bad[HEADER_LEN..MIN_BLOCK_LEN].copy_from_slice(&0u32.to_be_bytes());
	assert_eq!(parent.validate_block(&bad), Err(ExecutionError::InvalidEncoding));
	assert_eq!(parent, before);
	g.max_block_bytes = MIN_BLOCK_LEN as u32;
	let parent = Ledger::from_genesis(&g).unwrap();
	assert!(parent.prepare_block(vec![]).is_ok());
	assert_eq!(
		parent.prepare_block(vec![signed(parent.chain_id, "A", "B", 1, 0)]),
		Err(ExecutionError::BlockLimits)
	);
}
#[test]
fn transaction_errors_follow_block_order_including_decoding() {
	let parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
	let valid = parent
		.prepare_block(vec![
			signed(parent.chain_id, "A", "B", 1, 0),
			signed(parent.chain_id, "B", "C", 1, 0),
		])
		.unwrap();
	let mut block = valid.block;
	block.transfers[0] = signed(parent.chain_id, "A", "B", 101, 0);
	let mut bytes = block.encode().unwrap();
	// Later malformed profile must not mask an earlier financial failure.
	bytes[MIN_BLOCK_LEN + SIGNED_TRANSFER_LEN + 1] = 2;
	let commitment = hash(b"RINPQC/TXLIST/v1\0", &bytes[HEADER_LEN..]);
	bytes[76..108].copy_from_slice(&commitment);
	assert_eq!(
		parent.validate_block(&bytes),
		Err(ExecutionError::Payment { index: 0, reason: PaymentError::InsufficientFunds })
	);
}

#[test]
fn independently_encoded_payment_block_matches() {
	let f: serde_json::Value =
		serde_json::from_str(include_str!("../../../tests/fixtures/m1-payment-block.json"))
			.unwrap();
	let parent = Ledger::from_genesis(&fixture_genesis()).unwrap();
	let block = parent.prepare_block(vec![signed(parent.chain_id, "A", "B", 30, 0)]).unwrap();
	assert_eq!(hex::encode(block.block.encode().unwrap()), f["block_hex"]);
	assert_eq!(hex::encode(block.next.block_id), f["block_id"]);
	assert_eq!(hex::encode(block.next.state_root()), f["state_root"]);
	assert_eq!(
		parent.validate_block(&hex::decode(f["block_hex"].as_str().unwrap()).unwrap()).unwrap(),
		block
	);
}

#[test]
fn maximum_supply_and_nonce_increment_remain_checked() {
	let mut g = fixture_genesis();
	g.accounts = vec![(key("A").public_key(), u64::MAX)];
	let mut parent = Ledger::from_genesis(&g).unwrap();
	// Synthetic near-exhaustion snapshot for the arithmetic boundary.
	parent.accounts.get_mut(&key("A").public_key()).unwrap().next_nonce = u64::MAX - 1;
	let block = parent
		.prepare_block(vec![signed(parent.chain_id, "A", "B", u64::MAX, u64::MAX - 1)])
		.unwrap();
	assert_eq!(block.next.supply(), u64::MAX);
	assert_eq!(
		block.next.account(&key("A").public_key()),
		Account { balance: 0, next_nonce: u64::MAX }
	);
	assert_eq!(
		block.next.account(&key("B").public_key()),
		Account { balance: u64::MAX, next_nonce: 0 }
	);
	let receive = block.next.prepare_block(vec![signed(parent.chain_id, "B", "A", 1, 0)]).unwrap();
	assert_eq!(
		receive.next.account(&key("A").public_key()),
		Account { balance: 1, next_nonce: u64::MAX }
	);
	assert_eq!(
		receive.next.validate_transfer(&signed(parent.chain_id, "A", "B", 1, u64::MAX)),
		Err(PaymentError::NonceExhausted)
	);
}
