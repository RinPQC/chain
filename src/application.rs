//! Canonical payment bytes and deterministic speculative execution.
pub mod execution;
use crate::crypto::{hash, Error, Id, Result};

pub const VERSION: u16 = 1;
pub const SUITE: u16 = 1;
pub const UNSIGNED_TRANSFER_LEN: usize = 116;
pub const SIGNED_TRANSFER_LEN: usize = 180;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
	pub chain_id: Id,
	pub sender: Id,
	pub recipient: Id,
	pub amount: u64,
	pub nonce: u64,
}
impl Transfer {
	pub fn encode(&self) -> Vec<u8> {
		let mut out = Vec::with_capacity(UNSIGNED_TRANSFER_LEN);
		out.extend(VERSION.to_be_bytes());
		out.extend(SUITE.to_be_bytes());
		out.extend(self.chain_id);
		out.extend(self.sender);
		out.extend(self.recipient);
		out.extend(self.amount.to_be_bytes());
		out.extend(self.nonce.to_be_bytes());
		out
	}
	pub fn decode(bytes: &[u8]) -> Result<Self> {
		if bytes.len() != UNSIGNED_TRANSFER_LEN {
			return Err(Error::Encoding);
		}
		let mut c = Cursor::new(bytes);
		check_profile(c.u16()?, c.u16()?)?;
		Ok(Self {
			chain_id: c.read()?,
			sender: c.read()?,
			recipient: c.read()?,
			amount: c.u64()?,
			nonce: c.u64()?,
		})
	}
	pub fn signing_bytes(&self) -> Vec<u8> {
		[b"RINPQC/TRANSFER/v1\0".as_slice(), &self.encode()].concat()
	}
	pub fn id(&self) -> Id {
		hash(b"RINPQC/TXID/v1\0", &self.encode())
	}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedTransfer {
	pub transfer: Transfer,
	pub signature: [u8; 64],
}
impl SignedTransfer {
	pub fn encode(&self) -> Vec<u8> {
		let mut bytes = self.transfer.encode();
		bytes.extend(self.signature);
		bytes
	}
	pub fn decode(bytes: &[u8]) -> Result<Self> {
		if bytes.len() != SIGNED_TRANSFER_LEN {
			return Err(Error::Encoding);
		}
		Ok(Self {
			transfer: Transfer::decode(&bytes[..UNSIGNED_TRANSFER_LEN])?,
			signature: bytes[UNSIGNED_TRANSFER_LEN..].try_into().map_err(|_| Error::Encoding)?,
		})
	}
}

pub(crate) fn check_profile(version: u16, suite: u16) -> Result<()> {
	if version != VERSION {
		return Err(Error::Version);
	}
	if suite != SUITE {
		return Err(Error::Suite);
	}
	Ok(())
}
pub(crate) struct Cursor<'a> {
	rest: &'a [u8],
}
impl<'a> Cursor<'a> {
	pub fn new(rest: &'a [u8]) -> Self {
		Self { rest }
	}
	pub fn read<const N: usize>(&mut self) -> Result<[u8; N]> {
		if self.rest.len() < N {
			return Err(Error::Encoding);
		}
		let (head, tail) = self.rest.split_at(N);
		self.rest = tail;
		head.try_into().map_err(|_| Error::Encoding)
	}
	pub fn u16(&mut self) -> Result<u16> {
		Ok(u16::from_be_bytes(self.read()?))
	}
	pub fn u32(&mut self) -> Result<u32> {
		Ok(u32::from_be_bytes(self.read()?))
	}
	pub fn u64(&mut self) -> Result<u64> {
		Ok(u64::from_be_bytes(self.read()?))
	}
	pub fn is_empty(&self) -> bool {
		self.rest.is_empty()
	}
}
