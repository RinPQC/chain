use super::*;
use crate::{
	application::Transfer,
	crypto::{KeyRole, SecretKey, TransactionSigner},
};
use std::process::Command;

fn genesis() -> Genesis {
	let fixture: serde_json::Value =
		serde_json::from_str(include_str!("../../tests/fixtures/m1-identities.json")).unwrap();
	Genesis::from_json(&serde_json::to_vec(&fixture["genesis"]).unwrap()).unwrap()
}
fn block() -> Vec<u8> {
	let fixture: serde_json::Value =
		serde_json::from_str(include_str!("../../tests/fixtures/m1-payment-block.json")).unwrap();
	hex::decode(fixture["block_hex"].as_str().unwrap()).unwrap()
}
fn account(seed: u8) -> Id {
	SecretKey::from_seed(&[seed; 32], KeyRole::Transaction).public_key()
}
fn transaction_id() -> Id {
	Ledger::from_genesis(&genesis()).unwrap().validate_block(&block()).unwrap().outcomes()[0]
		.effect
		.tx_id
}
fn assert_state(store: &Store, committed: bool) {
	let state = store.ledger().unwrap();
	assert_eq!(state.height(), u64::from(committed));
	assert_eq!(
		state.account(&account(1)),
		Account { balance: if committed { 70 } else { 100 }, next_nonce: u64::from(committed) }
	);
	assert_eq!(
		state.account(&account(2)),
		Account { balance: if committed { 50 } else { 20 }, next_nonce: 0 }
	);
	assert_eq!(state.supply(), 120);
	assert_eq!(store.block(1).unwrap(), committed.then(block));
	let receipt = store.receipt(&transaction_id()).unwrap();
	assert_eq!(receipt.is_some(), committed);
	if let Some(receipt) = receipt {
		assert_eq!(receipt.outcome.height, 1);
		assert_eq!(receipt.outcome.index, 0);
		assert_eq!(receipt.block_id, state.block_id());
		assert_eq!(receipt.outcome.effect.sender_after, state.account(&account(1)));
		assert_eq!(receipt.outcome.effect.recipient_after, state.account(&account(2)));
	}
}
#[test]
fn restart_and_repeated_decisions_are_idempotent() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("ledger.redb");
	let mut store = Store::create(&path, &genesis()).unwrap();
	assert_state(&store, false);
	assert_eq!(store.commit_decided(1, &block()).unwrap(), Commit::Applied);
	assert_state(&store, true);
	drop(store);
	let mut store = Store::open(&path, &genesis()).unwrap();
	assert_state(&store, true);
	assert_eq!(store.commit_decided(1, &block()).unwrap(), Commit::AlreadyApplied);
	let empty = store.ledger().unwrap().prepare_block(vec![]).unwrap().block().encode().unwrap();
	store.commit_decided(2, &empty).unwrap();
	assert_eq!(store.commit_decided(1, &block()).unwrap(), Commit::AlreadyApplied);
	assert_eq!(store.receipt(&transaction_id()).unwrap().unwrap().outcome.height, 1);
	drop(store);
	let store = Store::open(&path, &genesis()).unwrap();
	assert_eq!(store.ledger().unwrap().height(), 2);
	assert_eq!(store.ledger().unwrap().account(&account(1)).balance, 70);
	assert_eq!(store.block(2).unwrap().unwrap(), empty);
}
#[test]
fn ordered_transfers_recover_intermediate_receipts_and_final_accounts() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("ledger.redb");
	let mut store = Store::create(&path, &genesis()).unwrap();
	let sender = SecretKey::from_seed(&[1; 32], KeyRole::Transaction);
	let chain_id = store.ledger().unwrap().chain_id();
	let transfers = (0..3)
		.map(|nonce| {
			sender
				.sign_transfer(
					Transfer {
						chain_id,
						sender: sender.public_key(),
						recipient: account(3),
						amount: 10,
						nonce,
					},
					&chain_id,
				)
				.unwrap()
		})
		.collect();
	let candidate = store.ledger().unwrap().prepare_block(transfers).unwrap();
	store.commit_decided(1, &candidate.block().encode().unwrap()).unwrap();
	drop(store);
	let store = Store::open(&path, &genesis()).unwrap();
	assert_eq!(store.ledger().unwrap(), candidate.candidate_state());
	for outcome in candidate.outcomes() {
		assert_eq!(store.receipt(&outcome.effect.tx_id).unwrap().unwrap().outcome, *outcome);
	}
}
#[test]
fn invalid_or_conflicting_blocks_do_not_change_state() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("ledger.redb");
	let mut store = Store::create(&path, &genesis()).unwrap();
	assert!(store.commit_decided(2, &block()).is_err());
	let mut invalid = block();
	*invalid.last_mut().unwrap() ^= 1;
	assert!(store.commit_decided(1, &invalid).is_err());
	assert_state(&store, false);
	store.commit_decided(1, &block()).unwrap();
	assert!(matches!(store.commit_decided(1, &invalid), Err(Error::Conflict)));
	assert!(store.commit_decided(0, &block()).is_err());
	assert_state(&store, true);
}
#[test]
fn existing_missing_empty_and_locked_files_fail_closed() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("ledger.redb");
	assert!(Store::open(&path, &genesis()).is_err());
	assert!(!path.exists());
	let store = Store::create(&path, &genesis()).unwrap();
	assert!(Store::open(&path, &genesis()).is_err());
	assert!(Store::create(&path, &genesis()).is_err());
	assert_state(&store, false);
	drop(store);
	let mut wrong = genesis();
	wrong.network_nonce[0] ^= 1;
	assert!(matches!(Store::open(&path, &wrong), Err(Error::Genesis)));
	let empty = dir.path().join("interrupted.redb");
	File::create(&empty).unwrap();
	assert!(Store::open(&empty, &genesis()).is_err());
	assert!(Store::create(&empty, &genesis()).is_err());
}
#[test]
fn corrupt_derived_rows_history_and_version_are_rejected() {
	for corruption in
		["version", "head", "balance", "nonce", "receipt", "block", "extra", "missing"]
	{
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("ledger.redb");
		let mut store = Store::create(&path, &genesis()).unwrap();
		store.commit_decided(1, &block()).unwrap();
		drop(store);
		let database = Database::open(&path).unwrap();
		let tx = database.begin_write().unwrap();
		{
			let mut table = tx.open_table(DATA).unwrap();
			let k = match corruption {
				"version" => b"version".to_vec(),
				"head" => b"head".to_vec(),
				"balance" | "nonce" => key(b'a', &account(1)),
				"receipt" => key(b'r', &transaction_id()),
				"block" | "missing" => key(b'b', &1u64.to_be_bytes()),
				"extra" => b"unexpected".to_vec(),
				_ => unreachable!(),
			};
			if corruption == "missing" {
				table.remove(k.as_slice()).unwrap();
			} else {
				let mut bytes =
					table.get(k.as_slice()).unwrap().map(|v| v.value().to_vec()).unwrap_or(vec![0]);
				let index = if corruption == "nonce" { 15 } else { 0 };
				bytes[index] ^= 1;
				table.insert(k.as_slice(), bytes.as_slice()).unwrap();
			}
		}
		tx.commit().unwrap();
		drop(database);
		assert!(Store::open(&path, &genesis()).is_err(), "accepted {corruption}");
	}
}
#[test]
fn interrupted_writer_cannot_serve_stale_memory() {
	let dir = tempfile::tempdir().unwrap();
	let path = dir.path().join("ledger.redb");
	let mut store = Store::create(&path, &genesis()).unwrap();
	let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
		store
			.commit_with_hook(1, &block(), |stage| {
				if stage == "after_commit" {
					panic!("simulated interrupted publication")
				}
			})
			.unwrap();
	}));
	assert!(result.is_err());
	assert!(matches!(store.ledger(), Err(Error::RecoveryRequired)));
	assert!(matches!(store.receipt(&transaction_id()), Err(Error::RecoveryRequired)));
	assert!(matches!(store.commit_decided(1, &block()), Err(Error::RecoveryRequired)));
	drop(store);
	assert_state(&Store::open(&path, &genesis()).unwrap(), true);
}
#[test]
fn process_exit_at_each_commit_boundary_recovers_all_or_nothing() {
	for stage in ["block", "debit", "credit", "receipt", "head", "before_commit", "after_commit"] {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("ledger.redb");
		drop(Store::create(&path, &genesis()).unwrap());
		let status = Command::new(std::env::current_exe().unwrap())
			.args(["--exact", "storage::tests::crash_worker", "--nocapture"])
			.env("RINPQC_CRASH_PATH", &path)
			.env("RINPQC_CRASH_STAGE", stage)
			.output()
			.unwrap()
			.status;
		assert_eq!(status.code(), Some(83), "worker did not reach {stage}");
		let mut store = Store::open(&path, &genesis()).unwrap();
		let committed = stage == "after_commit";
		assert_state(&store, committed);
		assert_eq!(
			store.commit_decided(1, &block()).unwrap(),
			if committed { Commit::AlreadyApplied } else { Commit::Applied }
		);
		assert_state(&store, true);
		drop(store);
		assert_state(&Store::open(&path, &genesis()).unwrap(), true);
	}
}
#[test]
fn crash_worker() {
	let Some(path) = std::env::var_os("RINPQC_CRASH_PATH") else {
		return;
	};
	let stage = std::env::var("RINPQC_CRASH_STAGE").unwrap();
	let mut store = Store::open(Path::new(&path), &genesis()).unwrap();
	store
		.commit_with_hook(1, &block(), |current| {
			// exit skips Rust destructors, so neither transactions nor the database close cleanly.
			if current == stage {
				std::process::exit(83);
			}
		})
		.unwrap();
	panic!("unreached fault boundary");
}
