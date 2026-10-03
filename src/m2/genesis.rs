use super::*;

pub const GENESIS_FIXED_LEN: usize = PREFIX_LEN + 32 + 4 + 4 + 8 + 4 * pq::PUBLIC_KEY_BYTES + 4;
pub const MAX_GENESIS_BYTES: usize = GENESIS_FIXED_LEN + MAX_ACCOUNTS * 56;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
	pub network_nonce: [u8; 32],
	pub max_block_bytes: u32,
	pub max_transactions: u32,
	pub target_interval_ms: u64,
	pub validators: [pq::PublicKey; 4],
	pub accounts: Vec<(AccountId, u64)>,
}
impl Genesis {
	/// Create a fresh network identity from OS entropy. Canonical input ordering is mandatory.
	pub fn fresh(
		validators: [pq::PublicKey; 4],
		accounts: Vec<(AccountId, u64)>,
		max_block_bytes: u32,
		max_transactions: u32,
		target_interval_ms: u64,
	) -> Result<Self> {
		use rand_core::{OsRng, RngCore};
		let mut value = Self {
			network_nonce: [0; 32],
			validators,
			accounts,
			max_block_bytes,
			max_transactions,
			target_interval_ms,
		};
		value.validate()?;
		OsRng
			.try_fill_bytes(&mut value.network_nonce)
			.map_err(|_| Error::Config("OS randomness unavailable"))?;
		Ok(value)
	}

	pub fn validate(&self) -> Result<()> {
		if !(wire::MIN_BLOCK_LEN..=MAX_BLOCK_BYTES).contains(&(self.max_block_bytes as usize))
			|| self.max_transactions as usize > MAX_TRANSACTIONS
			|| !(1..=60_000).contains(&self.target_interval_ms)
		{
			return Err(Error::Config("M2 genesis resource limits"));
		}
		let ids = self.validators.iter().map(ValidatorId::from_key).collect::<Result<Vec<_>>>()?;
		if ids.windows(2).any(|w| w[0] >= w[1]) {
			return Err(Error::Config("validators must be unique and sorted by validator ID"));
		}
		if self.accounts.is_empty()
			|| self.accounts.len() > MAX_ACCOUNTS
			|| self.accounts.windows(2).any(|w| w[0].0 >= w[1].0)
		{
			return Err(Error::Config("genesis allocations must be bounded, unique and sorted"));
		}
		let mut total = 0u64;
		for (_, balance) in &self.accounts {
			if *balance == 0 {
				return Err(Error::Config("zero genesis allocation"));
			}
			total = total.checked_add(*balance).ok_or(Error::Config("genesis supply overflow"))?;
		}
		Ok(())
	}
	pub fn encode(&self) -> Result<Vec<u8>> {
		self.validate()?;
		let mut out = prefix(1);
		out.extend(self.network_nonce);
		out.extend(self.max_block_bytes.to_be_bytes());
		out.extend(self.max_transactions.to_be_bytes());
		out.extend(self.target_interval_ms.to_be_bytes());
		for key in &self.validators {
			out.extend(key.to_bytes());
		}
		out.extend((self.accounts.len() as u32).to_be_bytes());
		for (id, balance) in &self.accounts {
			out.extend(id.0);
			out.extend(balance.to_be_bytes());
		}
		Ok(out)
	}
	pub fn decode(bytes: &[u8]) -> Result<Self> {
		let mut r = Reader::new(bytes, 1, MAX_GENESIS_BYTES)?;
		// Check the exact size/count before parsing keys or allocating account rows.
		if bytes.len() < GENESIS_FIXED_LEN {
			return Err(Error::Encoding);
		}
		let count = u32::from_be_bytes(
			bytes[GENESIS_FIXED_LEN - 4..GENESIS_FIXED_LEN]
				.try_into()
				.map_err(|_| Error::Encoding)?,
		) as usize;
		if count == 0 || count > MAX_ACCOUNTS || bytes.len() != GENESIS_FIXED_LEN + count * 56 {
			return Err(Error::Encoding);
		}
		let network_nonce = r.array()?;
		let max_block_bytes = r.u32()?;
		let max_transactions = r.u32()?;
		let target_interval_ms = r.u64()?;
		let mut key = || {
			pq::PublicKey::from_bytes(pq::SUITE, KeyRole::Validator, r.take(pq::PUBLIC_KEY_BYTES)?)
		};
		let validators = [key()?, key()?, key()?, key()?];
		r.u32()?;
		let mut accounts = Vec::with_capacity(count);
		for _ in 0..count {
			accounts.push((AccountId(r.array()?), r.u64()?));
		}
		r.finish()?;
		let value = Self {
			network_nonce,
			max_block_bytes,
			max_transactions,
			target_interval_ms,
			validators,
			accounts,
		};
		value.validate()?;
		Ok(value)
	}
	pub fn chain_id(&self) -> Result<ChainId> {
		Ok(ChainId(commitment(b"GENESIS", &self.encode()?)))
	}
	pub fn state_root(&self) -> Result<StateRoot> {
		self.validate()?;
		state_root(
			&self.accounts.iter().map(|(id, balance)| (*id, *balance, 0)).collect::<Vec<_>>(),
		)
	}
	/// Genesis is an out-of-band trust anchor; callers must pin its expected commitment.
	pub fn decode_for_chain(bytes: &[u8], expected: ChainId) -> Result<Self> {
		Reader::new(bytes, 1, MAX_GENESIS_BYTES)?;
		if ChainId(commitment(b"GENESIS", bytes)) != expected {
			return Err(Error::Chain);
		}
		Self::decode(bytes)
	}
}
