//! Pure payment execution. Valid proposals are not proof of consensus finality.
use crate::{
	application::{check_profile, Cursor, SignedTransfer, SIGNED_TRANSFER_LEN, SUITE, VERSION},
	crypto::{self, hash, Id},
	genesis::Genesis,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const HEADER_LEN: usize = 140;
pub const MIN_BLOCK_LEN: usize = HEADER_LEN + 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Account {
	pub balance: u64,
	pub next_nonce: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PaymentError {
	#[error("INVALID_ENCODING")]
	InvalidEncoding,
	#[error("UNSUPPORTED_VERSION")]
	UnsupportedVersion,
	#[error("UNSUPPORTED_SUITE")]
	UnsupportedSuite,
	#[error("WRONG_CHAIN")]
	WrongChain,
	#[error("INVALID_KEY")]
	InvalidKey,
	#[error("INVALID_SIGNATURE")]
	InvalidSignature,
	#[error("ZERO_AMOUNT")]
	ZeroAmount,
	#[error("SELF_TRANSFER")]
	SelfTransfer,
	#[error("NONCE_EXHAUSTED")]
	NonceExhausted,
	#[error("NONCE_TOO_LOW")]
	NonceTooLow,
	#[error("NONCE_TOO_HIGH")]
	NonceTooHigh,
	#[error("INSUFFICIENT_FUNDS")]
	InsufficientFunds,
	#[error("BALANCE_OVERFLOW")]
	BalanceOverflow,
	#[error("STATE_CAPACITY")]
	StateCapacity,
}
fn payment_error(error: crypto::Error) -> PaymentError {
	match error {
		crypto::Error::Version => PaymentError::UnsupportedVersion,
		crypto::Error::Suite => PaymentError::UnsupportedSuite,
		crypto::Error::Chain => PaymentError::WrongChain,
		crypto::Error::Key => PaymentError::InvalidKey,
		crypto::Error::Signature => PaymentError::InvalidSignature,
		_ => PaymentError::InvalidEncoding,
	}
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionError {
	#[error("invalid genesis")]
	InvalidGenesis,
	#[error("block exceeds byte or transaction limits")]
	BlockLimits,
	#[error("invalid canonical block encoding")]
	InvalidEncoding,
	#[error("unsupported block profile")]
	UnsupportedProfile,
	#[error("wrong block chain")]
	WrongChain,
	#[error("wrong block height or height exhausted")]
	WrongHeight,
	#[error("wrong parent block")]
	WrongParent,
	#[error("wrong transaction commitment")]
	TransactionsRoot,
	#[error("wrong state commitment")]
	StateRoot,
	#[error("transaction {index}: {reason}")]
	Payment { index: u32, reason: PaymentError },
	#[error("supply invariant failed")]
	SupplyInvariant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockHeader {
	pub chain_id: Id,
	pub height: u64,
	pub parent_id: Id,
	pub transactions_root: Id,
	pub state_root: Id,
}
impl BlockHeader {
	pub fn encode(&self) -> Vec<u8> {
		let mut bytes = Vec::with_capacity(HEADER_LEN);
		bytes.extend(VERSION.to_be_bytes());
		bytes.extend(SUITE.to_be_bytes());
		bytes.extend(self.chain_id);
		bytes.extend(self.height.to_be_bytes());
		bytes.extend(self.parent_id);
		bytes.extend(self.transactions_root);
		bytes.extend(self.state_root);
		bytes
	}
	pub fn id(&self) -> Id {
		hash(b"RINPQC/BLOCK/v1\0", &self.encode())
	}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
	pub header: BlockHeader,
	pub transfers: Vec<SignedTransfer>,
}
fn check_limits(
	count: usize,
	max_bytes: u32,
	max_transactions: u32,
) -> Result<usize, ExecutionError> {
	if count > max_transactions as usize || count > 4096 {
		return Err(ExecutionError::BlockLimits);
	}
	let len = MIN_BLOCK_LEN
		.checked_add(count.checked_mul(SIGNED_TRANSFER_LEN).ok_or(ExecutionError::BlockLimits)?)
		.ok_or(ExecutionError::BlockLimits)?;
	if len > max_bytes as usize || len > 1_048_576 {
		return Err(ExecutionError::BlockLimits);
	}
	Ok(len)
}
fn body(transfers: &[SignedTransfer]) -> Vec<u8> {
	// All callers check_limits before reaching this allocation/conversion.
	let mut bytes = Vec::with_capacity(4 + transfers.len() * SIGNED_TRANSFER_LEN);
	bytes.extend((transfers.len() as u32).to_be_bytes());
	for signed in transfers {
		bytes.extend(signed.encode());
	}
	bytes
}
impl Block {
	pub fn encode(&self) -> Result<Vec<u8>, ExecutionError> {
		check_limits(self.transfers.len(), 1_048_576, 4096)?;
		Ok([self.header.encode(), body(&self.transfers)].concat())
	}
}

/// Parent snapshot. Its provenance/finality must be established by the storage/consensus layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
	chain_id: Id,
	height: u64,
	block_id: Id,
	supply: u64,
	max_block_bytes: u32,
	max_transactions: u32,
	accounts: BTreeMap<Id, Account>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferEffect {
	pub tx_id: Id,
	pub sender: Id,
	pub recipient: Id,
	pub sender_after: Account,
	pub recipient_after: Account,
}
/// Speculative outcome, not an RPC finalized receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferOutcome {
	pub height: u64,
	pub index: u32,
	pub effect: TransferEffect,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBlock {
	block: Block,
	next: Ledger,
	outcomes: Vec<TransferOutcome>,
}
impl ValidatedBlock {
	pub fn block(&self) -> &Block {
		&self.block
	}
	pub fn candidate_state(&self) -> &Ledger {
		&self.next
	}
	pub fn outcomes(&self) -> &[TransferOutcome] {
		&self.outcomes
	}
}
impl Ledger {
	pub fn from_genesis(genesis: &Genesis) -> Result<Self, ExecutionError> {
		genesis.validate().map_err(|_| ExecutionError::InvalidGenesis)?;
		let chain_id = genesis.chain_id().map_err(|_| ExecutionError::InvalidGenesis)?;
		let supply = genesis
			.accounts
			.iter()
			.try_fold(0u64, |total, (_, balance)| total.checked_add(*balance))
			.ok_or(ExecutionError::InvalidGenesis)?;
		Ok(Self {
			chain_id,
			height: 0,
			block_id: chain_id,
			supply,
			max_block_bytes: genesis.max_block_bytes,
			max_transactions: genesis.max_transactions,
			accounts: genesis
				.accounts
				.iter()
				.map(|(key, balance)| (*key, Account { balance: *balance, next_nonce: 0 }))
				.collect(),
		})
	}
	pub fn chain_id(&self) -> Id {
		self.chain_id
	}
	pub fn height(&self) -> u64 {
		self.height
	}
	pub fn block_id(&self) -> Id {
		self.block_id
	}
	pub fn supply(&self) -> u64 {
		self.supply
	}
	pub fn account(&self, key: &Id) -> Account {
		self.accounts.get(key).copied().unwrap_or_default()
	}
	/// Ordered canonical rows; zero-balance historical accounts are retained.
	pub fn accounts(&self) -> impl Iterator<Item = (&Id, &Account)> {
		self.accounts.iter()
	}
	pub fn state_root(&self) -> Id {
		// Constructors and checked insertion maintain the u32 account-count bound.
		let mut h = Sha256::new();
		h.update(b"RINPQC/STATE/v1\0");
		h.update((self.accounts.len() as u32).to_be_bytes());
		for (key, account) in &self.accounts {
			h.update(key);
			h.update(account.balance.to_be_bytes());
			h.update(account.next_nonce.to_be_bytes());
		}
		h.finalize().into()
	}
	pub fn validate_transfer_bytes(&self, bytes: &[u8]) -> Result<TransferEffect, PaymentError> {
		let signed = SignedTransfer::decode(bytes).map_err(payment_error)?;
		self.validate_transfer(&signed)
	}
	pub fn validate_transfer(
		&self,
		signed: &SignedTransfer,
	) -> Result<TransferEffect, PaymentError> {
		crypto::verify_transfer(signed, &self.chain_id).map_err(payment_error)?;
		let tx = &signed.transfer;
		if tx.amount == 0 {
			return Err(PaymentError::ZeroAmount);
		}
		if tx.sender == tx.recipient {
			return Err(PaymentError::SelfTransfer);
		}
		let sender = self.account(&tx.sender);
		let recipient = self.account(&tx.recipient);
		let next_nonce = sender.next_nonce.checked_add(1).ok_or(PaymentError::NonceExhausted)?;
		if tx.nonce < sender.next_nonce {
			return Err(PaymentError::NonceTooLow);
		}
		if tx.nonce > sender.next_nonce {
			return Err(PaymentError::NonceTooHigh);
		}
		let debit = sender.balance.checked_sub(tx.amount).ok_or(PaymentError::InsufficientFunds)?;
		let credit =
			recipient.balance.checked_add(tx.amount).ok_or(PaymentError::BalanceOverflow)?;
		check_capacity(self.accounts.len(), self.accounts.contains_key(&tx.recipient))?;
		Ok(TransferEffect {
			tx_id: tx.id(),
			sender: tx.sender,
			recipient: tx.recipient,
			sender_after: Account { balance: debit, next_nonce },
			recipient_after: Account { balance: credit, next_nonce: recipient.next_nonce },
		})
	}
	fn execute<I>(
		&self,
		count: usize,
		transfers: I,
	) -> Result<(Self, Vec<SignedTransfer>, Vec<TransferOutcome>), ExecutionError>
	where
		I: IntoIterator<Item = Result<SignedTransfer, PaymentError>>,
	{
		check_limits(count, self.max_block_bytes, self.max_transactions)?;
		let height = self.height.checked_add(1).ok_or(ExecutionError::WrongHeight)?;
		let mut next = self.clone();
		let mut outcomes = Vec::with_capacity(count);
		let mut decoded = Vec::with_capacity(count);
		for (index, item) in transfers.into_iter().enumerate() {
			if index >= count {
				return Err(ExecutionError::InvalidEncoding);
			}
			let signed =
				item.map_err(|reason| ExecutionError::Payment { index: index as u32, reason })?;
			let effect = next
				.validate_transfer(&signed)
				.map_err(|reason| ExecutionError::Payment { index: index as u32, reason })?;
			next.accounts.insert(effect.sender, effect.sender_after);
			next.accounts.insert(effect.recipient, effect.recipient_after);
			outcomes.push(TransferOutcome { height, index: index as u32, effect });
			decoded.push(signed);
		}
		if decoded.len() != count {
			return Err(ExecutionError::InvalidEncoding);
		}
		let sum = next.accounts.values().try_fold(0u64, |total, a| total.checked_add(a.balance));
		if sum != Some(self.supply) {
			return Err(ExecutionError::SupplyInvariant);
		}
		next.height = height;
		Ok((next, decoded, outcomes))
	}
	pub fn prepare_block(
		&self,
		transfers: Vec<SignedTransfer>,
	) -> Result<ValidatedBlock, ExecutionError> {
		let (mut next, transfers, outcomes) =
			self.execute(transfers.len(), transfers.into_iter().map(Ok))?;
		let header = BlockHeader {
			chain_id: self.chain_id,
			height: next.height,
			parent_id: self.block_id,
			transactions_root: hash(b"RINPQC/TXLIST/v1\0", &body(&transfers)),
			state_root: next.state_root(),
		};
		next.block_id = header.id();
		Ok(ValidatedBlock { block: Block { header, transfers }, next, outcomes })
	}
	pub fn validate_block(&self, bytes: &[u8]) -> Result<ValidatedBlock, ExecutionError> {
		if bytes.len() < MIN_BLOCK_LEN {
			return Err(ExecutionError::InvalidEncoding);
		}
		if bytes.len() > self.max_block_bytes as usize {
			return Err(ExecutionError::BlockLimits);
		}
		let mut c = Cursor::new(bytes);
		let version = c.u16().map_err(|_| ExecutionError::InvalidEncoding)?;
		let suite = c.u16().map_err(|_| ExecutionError::InvalidEncoding)?;
		check_profile(version, suite).map_err(|_| ExecutionError::UnsupportedProfile)?;
		let mut decode = || -> crypto::Result<BlockHeader> {
			Ok(BlockHeader {
				chain_id: c.read()?,
				height: c.u64()?,
				parent_id: c.read()?,
				transactions_root: c.read()?,
				state_root: c.read()?,
			})
		};
		let header = decode().map_err(|_| ExecutionError::InvalidEncoding)?;
		let count = c.u32().map_err(|_| ExecutionError::InvalidEncoding)? as usize;
		let expected_len = check_limits(count, self.max_block_bytes, self.max_transactions)?;
		if bytes.len() != expected_len {
			return Err(ExecutionError::InvalidEncoding);
		}
		if header.chain_id != self.chain_id {
			return Err(ExecutionError::WrongChain);
		}
		if Some(header.height) != self.height.checked_add(1) {
			return Err(ExecutionError::WrongHeight);
		}
		if header.parent_id != self.block_id {
			return Err(ExecutionError::WrongParent);
		}
		if header.transactions_root != hash(b"RINPQC/TXLIST/v1\0", &bytes[HEADER_LEN..]) {
			return Err(ExecutionError::TransactionsRoot);
		}
		let decoded = bytes[MIN_BLOCK_LEN..]
			.chunks_exact(SIGNED_TRANSFER_LEN)
			.map(|raw| SignedTransfer::decode(raw).map_err(payment_error));
		let (mut next, transfers, outcomes) = self.execute(count, decoded)?;
		if next.state_root() != header.state_root {
			return Err(ExecutionError::StateRoot);
		}
		next.block_id = header.id();
		Ok(ValidatedBlock { block: Block { header, transfers }, next, outcomes })
	}
}
fn check_capacity(count: usize, recipient_exists: bool) -> Result<(), PaymentError> {
	if !recipient_exists && count >= u32::MAX as usize {
		return Err(PaymentError::StateCapacity);
	}
	Ok(())
}
#[cfg(test)]
mod tests;
