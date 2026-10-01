//! Canonical, fixed-membership M1 genesis and initial state commitments.
use crate::{
	application::{check_profile, Cursor, SUITE, VERSION},
	crypto::{hash, public_key, Error, Id, Result},
};
use serde::{Deserialize, Serialize};

pub const MAX_GENESIS_JSON: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Genesis {
	pub network_nonce: Id,
	pub max_block_bytes: u32,
	pub max_transactions: u32,
	pub target_interval_ms: u64,
	pub validators: [Id; 4],
	pub accounts: Vec<(Id, u64)>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenesisFile {
	version: u16,
	suite: u16,
	network_nonce: String,
	max_block_bytes: u32,
	max_transactions: u32,
	target_interval_ms: String,
	validators: [String; 4],
	accounts: Vec<Allocation>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Allocation {
	account_id: String,
	balance: String,
}

pub fn parse_id(text: &str) -> Result<Id> {
	if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
		return Err(Error::Encoding);
	}
	let mut out = [0; 32];
	hex::decode_to_slice(text, &mut out).map_err(|_| Error::Encoding)?;
	Ok(out)
}
fn decimal(text: &str) -> Result<u64> {
	if text.is_empty()
		|| text.len() > 20
		|| !text.bytes().all(|b| b.is_ascii_digit())
		|| (text.len() > 1 && text.starts_with('0'))
	{
		return Err(Error::Encoding);
	}
	text.parse().map_err(|_| Error::Encoding)
}
impl Genesis {
	pub fn validate(&self) -> Result<()> {
		if !(144..=1_048_576).contains(&self.max_block_bytes)
			|| self.max_transactions > 4096
			|| !(1..=60_000).contains(&self.target_interval_ms)
		{
			return Err(Error::Config("genesis resource limits"));
		}
		for key in &self.validators {
			public_key(key)?;
		}
		if self.validators.windows(2).any(|w| w[0] >= w[1]) {
			return Err(Error::Config("validators must be unique and sorted"));
		}
		if self.accounts.is_empty() || self.accounts.len() > 65_536 {
			return Err(Error::Config("genesis account count"));
		}
		let mut total = 0u128;
		for (key, balance) in &self.accounts {
			public_key(key)?;
			if *balance == 0 {
				return Err(Error::Config("zero genesis allocation"));
			}
			total += u128::from(*balance);
		}
		if total > u128::from(u64::MAX) {
			return Err(Error::Config("genesis supply overflow"));
		}
		if self.accounts.windows(2).any(|w| w[0].0 >= w[1].0) {
			return Err(Error::Config("accounts must be unique and sorted"));
		}
		Ok(())
	}
	pub fn encode(&self) -> Result<Vec<u8>> {
		self.validate()?;
		let mut out = Vec::with_capacity(184 + 40 * self.accounts.len());
		out.extend(VERSION.to_be_bytes());
		out.extend(SUITE.to_be_bytes());
		out.extend(self.network_nonce);
		out.extend(self.max_block_bytes.to_be_bytes());
		out.extend(self.max_transactions.to_be_bytes());
		out.extend(self.target_interval_ms.to_be_bytes());
		for key in self.validators {
			out.extend(key);
		}
		out.extend((self.accounts.len() as u32).to_be_bytes());
		for (key, balance) in &self.accounts {
			out.extend(key);
			out.extend(balance.to_be_bytes());
		}
		Ok(out)
	}
	pub fn decode(bytes: &[u8]) -> Result<Self> {
		if bytes.len() > 184 + 40 * 65_536 {
			return Err(Error::Encoding);
		}
		let mut c = Cursor::new(bytes);
		check_profile(c.u16()?, c.u16()?)?;
		let network_nonce = c.read()?;
		let max_block_bytes = c.u32()?;
		let max_transactions = c.u32()?;
		let target_interval_ms = c.u64()?;
		let validators = [c.read()?, c.read()?, c.read()?, c.read()?];
		let count = c.u32()? as usize;
		if count == 0 || count > 65_536 || bytes.len() != 184 + 40 * count {
			return Err(Error::Encoding);
		}
		let mut accounts = Vec::with_capacity(count);
		for _ in 0..count {
			accounts.push((c.read()?, c.u64()?));
		}
		if !c.is_empty() {
			return Err(Error::Encoding);
		}
		let genesis = Self {
			network_nonce,
			max_block_bytes,
			max_transactions,
			target_interval_ms,
			validators,
			accounts,
		};
		genesis.validate()?;
		Ok(genesis)
	}
	pub fn chain_id(&self) -> Result<Id> {
		Ok(hash(b"RINPQC/GENESIS/v1\0", &self.encode()?))
	}
	pub fn state_root(&self) -> Result<Id> {
		self.validate()?;
		let mut bytes = Vec::with_capacity(4 + self.accounts.len() * 48);
		bytes.extend((self.accounts.len() as u32).to_be_bytes());
		for (key, balance) in &self.accounts {
			bytes.extend(key);
			bytes.extend(balance.to_be_bytes());
			bytes.extend(0u64.to_be_bytes());
		}
		Ok(hash(b"RINPQC/STATE/v1\0", &bytes))
	}
	pub fn from_json(bytes: &[u8]) -> Result<Self> {
		if bytes.len() > MAX_GENESIS_JSON {
			return Err(Error::Encoding);
		}
		let f: GenesisFile = serde_json::from_slice(bytes).map_err(|_| Error::Encoding)?;
		check_profile(f.version, f.suite)?;
		let mut validators = [[0; 32]; 4];
		for (out, text) in validators.iter_mut().zip(&f.validators) {
			*out = parse_id(text)?;
		}
		let accounts = f
			.accounts
			.iter()
			.map(|a| Ok((parse_id(&a.account_id)?, decimal(&a.balance)?)))
			.collect::<Result<_>>()?;
		let genesis = Self {
			network_nonce: parse_id(&f.network_nonce)?,
			max_block_bytes: f.max_block_bytes,
			max_transactions: f.max_transactions,
			target_interval_ms: decimal(&f.target_interval_ms)?,
			validators,
			accounts,
		};
		genesis.validate()?;
		Ok(genesis)
	}
	pub fn to_json(&self) -> Result<Vec<u8>> {
		self.validate()?;
		let f = GenesisFile {
			version: VERSION,
			suite: SUITE,
			network_nonce: hex::encode(self.network_nonce),
			max_block_bytes: self.max_block_bytes,
			max_transactions: self.max_transactions,
			target_interval_ms: self.target_interval_ms.to_string(),
			validators: self.validators.map(hex::encode),
			accounts: self
				.accounts
				.iter()
				.map(|(key, balance)| Allocation {
					account_id: hex::encode(key),
					balance: balance.to_string(),
				})
				.collect(),
		};
		serde_json::to_vec_pretty(&f).map_err(|_| Error::Encoding)
	}
}
