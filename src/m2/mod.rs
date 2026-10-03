//! Canonical M2 formats. Runtime adapters remain M1 until their dedicated integration tasks.
use crate::crypto::{pq, Error, KeyRole, Result};
use sha2::{Digest, Sha384};

pub mod genesis;
pub mod storage;
pub mod wire;

pub const VERSION: u16 = 2;
/// Fixed profile: Pure ML-DSA-87 signatures and SHA-384 application commitments.
pub const PROFILE: u16 = 1;
pub const PREFIX_LEN: usize = 9;
pub const MAX_ACCOUNTS: usize = 65_536;
pub const MAX_BLOCK_BYTES: usize = 1_048_576;
pub const MAX_TRANSACTIONS: usize = 4096;

fn commitment_hasher(domain: &[u8], len: u64) -> Sha384 {
	let mut h = Sha384::new();
	h.update(b"RINPQC/M2/HASH");
	h.update(VERSION.to_be_bytes());
	h.update(PROFILE.to_be_bytes());
	h.update((domain.len() as u16).to_be_bytes());
	h.update(domain);
	h.update(len.to_be_bytes());
	h
}
fn commitment(domain: &[u8], payload: &[u8]) -> [u8; 48] {
	let mut h = commitment_hasher(domain, payload.len() as u64);
	h.update(payload);
	h.finalize().into()
}
macro_rules! identifier {
	($name:ident) => {
		#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
		pub struct $name(pub [u8; 48]);
		impl $name {
			pub fn to_hex(self) -> String {
				hex::encode(self.0)
			}
			pub fn from_hex(text: &str) -> Result<Self> {
				if text.len() != 96
					|| !text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
				{
					return Err(Error::Encoding);
				}
				let mut bytes = [0; 48];
				hex::decode_to_slice(text, &mut bytes).map_err(|_| Error::Encoding)?;
				Ok(Self(bytes))
			}
		}
	};
}
identifier!(AccountId);
identifier!(ValidatorId);
identifier!(ChainId);
identifier!(TransactionId);
identifier!(BlockId);
identifier!(StateRoot);
identifier!(TransactionsRoot);

fn key_id(key: &pq::PublicKey, role: KeyRole, domain: &[u8]) -> Result<[u8; 48]> {
	if key.role() != role {
		return Err(Error::Role);
	}
	let mut bytes = Vec::with_capacity(2 + pq::PUBLIC_KEY_BYTES);
	bytes.extend(pq::SUITE.to_be_bytes());
	bytes.extend(key.to_bytes());
	Ok(commitment(domain, &bytes))
}
impl AccountId {
	pub fn from_key(key: &pq::PublicKey) -> Result<Self> {
		Ok(Self(key_id(key, KeyRole::Transaction, b"ACCOUNT")?))
	}
}
impl ValidatorId {
	pub fn from_key(key: &pq::PublicKey) -> Result<Self> {
		Ok(Self(key_id(key, KeyRole::Validator, b"VALIDATOR")?))
	}
}

fn prefix(tag: u8) -> Vec<u8> {
	let mut out = Vec::with_capacity(PREFIX_LEN);
	out.extend(b"RIN2");
	out.extend(VERSION.to_be_bytes());
	out.extend(PROFILE.to_be_bytes());
	out.push(tag);
	out
}
struct Reader<'a> {
	rest: &'a [u8],
}
impl<'a> Reader<'a> {
	fn new(bytes: &'a [u8], tag: u8, max: usize) -> Result<Self> {
		if bytes.len() < PREFIX_LEN || bytes.len() > max || &bytes[..4] != b"RIN2" {
			return Err(Error::Encoding);
		}
		if bytes[4..6] != VERSION.to_be_bytes() {
			return Err(Error::Version);
		}
		if bytes[6..8] != PROFILE.to_be_bytes() {
			return Err(Error::Suite);
		}
		if bytes[8] != tag {
			return Err(Error::Encoding);
		}
		Ok(Self { rest: &bytes[PREFIX_LEN..] })
	}
	fn take(&mut self, len: usize) -> Result<&'a [u8]> {
		if self.rest.len() < len {
			return Err(Error::Encoding);
		}
		let (head, tail) = self.rest.split_at(len);
		self.rest = tail;
		Ok(head)
	}
	fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
		self.take(N)?.try_into().map_err(|_| Error::Encoding)
	}
	fn u8(&mut self) -> Result<u8> {
		Ok(self.array::<1>()?[0])
	}
	fn u32(&mut self) -> Result<u32> {
		Ok(u32::from_be_bytes(self.array()?))
	}
	fn u64(&mut self) -> Result<u64> {
		Ok(u64::from_be_bytes(self.array()?))
	}
	fn chain(&mut self, expected: ChainId) -> Result<()> {
		if self.array::<48>()? != expected.0 {
			return Err(Error::Chain);
		}
		Ok(())
	}
	fn finish(self) -> Result<()> {
		if !self.rest.is_empty() {
			return Err(Error::Encoding);
		}
		Ok(())
	}
}

/// Sorted account rows, including retained zero-balance accounts. No balances are modified here.
pub fn state_root(rows: &[(AccountId, u64, u64)]) -> Result<StateRoot> {
	let count = u32::try_from(rows.len()).map_err(|_| Error::Encoding)?;
	if rows.windows(2).any(|w| w[0].0 >= w[1].0) {
		return Err(Error::Encoding);
	}
	let mut h = commitment_hasher(b"STATE", 4 + u64::from(count) * 64);
	h.update(count.to_be_bytes());
	for (id, balance, nonce) in rows {
		h.update(id.0);
		h.update(balance.to_be_bytes());
		h.update(nonce.to_be_bytes());
	}
	Ok(StateRoot(h.finalize().into()))
}
#[cfg(test)]
mod tests;
