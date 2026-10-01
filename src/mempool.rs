//! Volatile local policy; admission is neither block validity nor finality.
use crate::{
	application::{
		execution::{ExecutionError, Ledger, PaymentError, ValidatedBlock},
		SignedTransfer, SIGNED_TRANSFER_LEN,
	},
	crypto::Id,
};
use std::collections::BTreeMap;

/// Fixed M1 bound: at most 737,280 canonical payment bytes, plus bounded index overhead.
pub const MAX_PENDING: usize = 4096;
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionError {
	#[error("REQUEST_TOO_LARGE")]
	RequestTooLarge,
	#[error("{0}")]
	Invalid(#[from] PaymentError),
	#[error("NONCE_CONFLICT")]
	NonceConflict,
	#[error("QUEUE_FULL")]
	QueueFull,
}
#[derive(Default)]
pub struct PaymentQueue {
	// One current-nonce instruction per sender. Ordered keys also define proposal order.
	entries: BTreeMap<Id, SignedTransfer>,
}
impl PaymentQueue {
	pub fn len(&self) -> usize {
		self.entries.len()
	}
	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}
	pub fn admit(&mut self, ledger: &Ledger, raw: &[u8]) -> Result<Id, AdmissionError> {
		if raw.len() > SIGNED_TRANSFER_LEN {
			return Err(AdmissionError::RequestTooLarge);
		}
		// Validate even a duplicate ID: IDs do not commit to signatures.
		let effect = ledger.validate_transfer_bytes(raw)?;
		let signed = SignedTransfer::decode(raw).map_err(|_| PaymentError::InvalidEncoding)?;
		if let Some(existing) = self.entries.get(&effect.sender) {
			return if existing.transfer.id() == effect.tx_id {
				Ok(effect.tx_id)
			} else {
				Err(AdmissionError::NonceConflict)
			};
		}
		if self.len() >= MAX_PENDING {
			return Err(AdmissionError::QueueFull);
		}
		self.entries.insert(effect.sender, signed);
		Ok(effect.tx_id)
	}
	/// Call after each durable commit, including recovery before admitting startup input.
	/// No replacement or age eviction: reject new entries when full.
	pub fn revalidate(&mut self, ledger: &Ledger) {
		self.entries.retain(|_, signed| ledger.validate_transfer(signed).is_ok());
	}
	pub fn propose(&self, ledger: &Ledger) -> Result<ValidatedBlock, ExecutionError> {
		ledger.assemble_block(self.entries.values())
	}
	/// Rotate without an unbounded retry list. Proposal construction does not remove entries.
	pub fn next_gossip(&self, after: Option<Id>) -> Option<(Id, &SignedTransfer)> {
		use std::ops::Bound::{Excluded, Unbounded};
		let next = after.and_then(|key| self.entries.range((Excluded(key), Unbounded)).next());
		next.or_else(|| self.entries.first_key_value()).map(|(key, value)| (*key, value))
	}
}
#[cfg(test)]
mod tests;
