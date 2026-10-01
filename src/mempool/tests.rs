use super::*;
use crate::{
	application::Transfer,
	crypto::{KeyRole, SecretKey, TransactionSigner},
	genesis::Genesis,
};
fn key(i: u32) -> SecretKey {
	let mut seed = [0; 32];
	seed[..4].copy_from_slice(&i.to_be_bytes());
	SecretKey::from_seed(&seed, KeyRole::Transaction)
}
fn ledger(count: u32, bytes: u32, transactions: u32) -> Ledger {
	let mut validators = [
		key(10001).public_key(),
		key(10002).public_key(),
		key(10003).public_key(),
		key(10004).public_key(),
	];
	validators.sort();
	let mut accounts: Vec<_> = (0..count).map(|i| (key(i).public_key(), 100)).collect();
	accounts.sort();
	Ledger::from_genesis(&Genesis {
		network_nonce: [7; 32],
		max_block_bytes: bytes,
		max_transactions: transactions,
		target_interval_ms: 5000,
		validators,
		accounts,
	})
	.unwrap()
}
fn payment(l: &Ledger, sender: u32, amount: u64, nonce: u64) -> SignedTransfer {
	let key = key(sender);
	key.sign_transfer(
		Transfer {
			chain_id: l.chain_id(),
			sender: key.public_key(),
			recipient: self::key(9000).public_key(),
			amount,
			nonce,
		},
		&l.chain_id(),
	)
	.unwrap()
}
#[test]
fn admission_rejects_conflicts_gaps_forged_duplicates_and_oversized_requests() {
	let l = ledger(2, 1048576, 4096);
	let mut q = PaymentQueue::default();
	let p = payment(&l, 0, 30, 0);
	assert_eq!(q.admit(&l, &p.encode()), Ok(p.transfer.id()));
	for _ in 0..100 {
		assert_eq!(q.admit(&l, &p.encode()), Ok(p.transfer.id()));
	}
	assert_eq!(q.len(), 1);
	let mut forged = p.clone();
	forged.signature[0] ^= 1;
	assert_eq!(
		q.admit(&l, &forged.encode()),
		Err(AdmissionError::Invalid(PaymentError::InvalidSignature))
	);
	assert_eq!(q.admit(&l, &payment(&l, 0, 40, 0).encode()), Err(AdmissionError::NonceConflict));
	assert_eq!(
		q.admit(&l, &payment(&l, 1, 30, 1).encode()),
		Err(AdmissionError::Invalid(PaymentError::NonceTooHigh))
	);
	assert_eq!(q.admit(&l, &[0; 181]), Err(AdmissionError::RequestTooLarge));
	assert_eq!(q.admit(&l, &[0; 179]), Err(AdmissionError::Invalid(PaymentError::InvalidEncoding)));
}
#[test]
fn full_queue_preserves_existing_entries_and_accepts_exact_retries() {
	let l = ledger(4097, 1048576, 4096);
	let mut q = PaymentQueue::default();
	for i in 0..4096 {
		q.admit(&l, &payment(&l, i, 1, 0).encode()).unwrap();
	}
	assert_eq!(q.len(), MAX_PENDING);
	assert_eq!(q.admit(&l, &payment(&l, 4096, 1, 0).encode()), Err(AdmissionError::QueueFull));
	q.admit(&l, &payment(&l, 0, 1, 0).encode()).unwrap();
	assert_eq!(q.len(), MAX_PENDING);
}
#[test]
fn proposals_fit_each_limit_retain_failed_round_entries_and_revalidate_after_commit() {
	for (bytes, count, expected) in [(144, 3, 0), (324, 3, 1), (1048576, 2, 2)] {
		let l = ledger(3, bytes, count);
		let mut q = PaymentQueue::default();
		for i in 0..3 {
			q.admit(&l, &payment(&l, i, 30, 0).encode()).unwrap();
		}
		let candidate = q.propose(&l).unwrap();
		assert_eq!(candidate.block().transfers.len(), expected);
		let raw = candidate.block().encode().unwrap();
		assert!(raw.len() <= bytes as usize);
		assert_eq!(l.validate_block(&raw).unwrap(), candidate);
		assert_eq!(q.propose(&l).unwrap(), candidate);
		assert_eq!(q.len(), 3);
		q.revalidate(candidate.candidate_state());
		assert_eq!(q.len(), 3 - expected);
	}
	let l = ledger(2, 1048576, 4096);
	let mut q = PaymentQueue::default();
	q.admit(&l, &payment(&l, 0, 30, 0).encode()).unwrap();
	q.admit(&l, &payment(&l, 1, 30, 0).encode()).unwrap();
	let other = l.prepare_block(vec![payment(&l, 0, 60, 0)]).unwrap();
	q.revalidate(other.candidate_state());
	assert_eq!(q.len(), 1);
	let candidate = q.propose(other.candidate_state()).unwrap();
	assert_eq!(candidate.block().transfers[0].transfer.sender, key(1).public_key());
	let (cursor, _) = q.next_gossip(None).unwrap();
	assert_eq!(q.next_gossip(Some(cursor)).unwrap().0, cursor);
}
