//! Explicit M1 signing roles. No durable consensus signing is enabled here.
pub mod pq;

use crate::application::{SignedTransfer, Transfer, SUITE, VERSION};
use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub type Id = [u8; 32];
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("invalid canonical encoding")]
	Encoding,
	#[error("unsupported version")]
	Version,
	#[error("unsupported cryptographic suite")]
	Suite,
	#[error("invalid public key")]
	Key,
	#[error("invalid signature")]
	Signature,
	#[error("wrong chain")]
	Chain,
	#[error("wrong key role or signer")]
	Role,
	#[error("invalid configuration: {0}")]
	Config(&'static str),
	#[error("filesystem operation failed: {0}")]
	Io(#[from] std::io::Error),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyRole {
	Transaction = 1,
	Validator = 2,
	Network = 3,
}
impl TryFrom<u8> for KeyRole {
	type Error = Error;
	fn try_from(value: u8) -> Result<Self> {
		match value {
			1 => Ok(Self::Transaction),
			2 => Ok(Self::Validator),
			3 => Ok(Self::Network),
			_ => Err(Error::Role),
		}
	}
}
// Deliberately no Debug, Clone or public secret export.
pub struct SecretKey {
	key: SigningKey,
	role: KeyRole,
}
impl SecretKey {
	pub fn generate(role: KeyRole) -> Result<Self> {
		let mut seed = Zeroizing::new([0u8; 32]);
		OsRng
			.try_fill_bytes(seed.as_mut())
			.map_err(|_| Error::Config("OS randomness unavailable"))?;
		Ok(Self::from_seed(&seed, role))
	}
	pub fn from_seed(seed: &[u8; 32], role: KeyRole) -> Self {
		Self { key: SigningKey::from_bytes(seed), role }
	}
	pub fn public_key(&self) -> Id {
		self.key.verifying_key().to_bytes()
	}
	pub fn role(&self) -> KeyRole {
		self.role
	}
	pub(crate) fn seed(&self) -> Zeroizing<[u8; 32]> {
		Zeroizing::new(self.key.to_bytes())
	}
}

pub fn public_key(bytes: &Id) -> Result<VerifyingKey> {
	let point = CompressedEdwardsY(*bytes).decompress().ok_or(Error::Key)?;
	if point.compress().to_bytes() != *bytes || point.is_small_order() || !point.is_torsion_free() {
		return Err(Error::Key);
	}
	VerifyingKey::from_bytes(bytes).map_err(|_| Error::Key)
}
pub fn hash(domain: &[u8], bytes: &[u8]) -> Id {
	let mut h = Sha256::new();
	h.update(domain);
	h.update(bytes);
	h.finalize().into()
}
pub fn verify_signature(key: &Id, bytes: &[u8], signature: &[u8; 64]) -> Result<()> {
	public_key(key)?
		.verify_strict(bytes, &Signature::from_bytes(signature))
		.map_err(|_| Error::Signature)
}

pub trait TransactionSigner {
	fn sign_transfer(&self, transfer: Transfer, expected_chain: &Id) -> Result<SignedTransfer>;
}
impl TransactionSigner for SecretKey {
	fn sign_transfer(&self, transfer: Transfer, expected_chain: &Id) -> Result<SignedTransfer> {
		if self.role != KeyRole::Transaction || transfer.sender != self.public_key() {
			return Err(Error::Role);
		}
		if &transfer.chain_id != expected_chain {
			return Err(Error::Chain);
		}
		public_key(&transfer.recipient)?;
		let signature = self.key.sign(&transfer.signing_bytes()).to_bytes();
		Ok(SignedTransfer { transfer, signature })
	}
}
pub fn verify_transfer(signed: &SignedTransfer, expected_chain: &Id) -> Result<()> {
	if &signed.transfer.chain_id != expected_chain {
		return Err(Error::Chain);
	}
	public_key(&signed.transfer.sender)?;
	public_key(&signed.transfer.recipient)?;
	verify_signature(&signed.transfer.sender, &signed.transfer.signing_bytes(), &signed.signature)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ConsensusKind {
	Proposal = 1,
	Prevote = 2,
	Precommit = 3,
}
#[derive(Clone, Debug)]
pub struct ConsensusMessage {
	pub chain_id: Id,
	pub height: u64,
	pub round: u32,
	pub kind: ConsensusKind,
	pub value: Option<Id>,
}
impl ConsensusMessage {
	pub fn signing_bytes(&self) -> Result<Vec<u8>> {
		if self.height == 0
			|| self.round > i32::MAX as u32
			|| (self.kind == ConsensusKind::Proposal && self.value.is_none())
		{
			return Err(Error::Config("invalid consensus signing context"));
		}
		let mut out = b"RINPQC/CONSENSUS/v1\0".to_vec();
		out.extend(VERSION.to_be_bytes());
		out.extend(SUITE.to_be_bytes());
		out.extend(self.chain_id);
		out.push(self.kind as u8);
		out.extend(self.height.to_be_bytes());
		out.extend(self.round.to_be_bytes());
		out.push(u8::from(self.value.is_some()));
		if let Some(value) = self.value {
			out.extend(value);
		}
		Ok(out)
	}
}
/// Cryptographic primitive only: the engine adapter must enforce durable anti-equivocation in #9.
pub trait ConsensusSigner {
	fn sign_consensus(&self, message: &ConsensusMessage, expected_chain: &Id) -> Result<[u8; 64]>;
}
impl ConsensusSigner for SecretKey {
	fn sign_consensus(&self, message: &ConsensusMessage, expected_chain: &Id) -> Result<[u8; 64]> {
		if self.role != KeyRole::Validator {
			return Err(Error::Role);
		}
		if &message.chain_id != expected_chain {
			return Err(Error::Chain);
		}
		Ok(self.key.sign(&message.signing_bytes()?).to_bytes())
	}
}
pub fn verify_consensus(
	key: &Id,
	message: &ConsensusMessage,
	expected_chain: &Id,
	signature: &[u8; 64],
) -> Result<()> {
	if &message.chain_id != expected_chain {
		return Err(Error::Chain);
	}
	verify_signature(key, &message.signing_bytes()?, signature)
}
