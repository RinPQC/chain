//! M2 Pure ML-DSA-87 primitives. The M1 node does not use these yet.
//!
//! Callers supply canonical, chain/profile-bound message bytes. Consensus callers must look up
//! persisted authorizations BEFORE signing: a new signature uses fresh entropy, even for a retry.
//! Clone an `Arc<SecretKey>` when a shared signer is needed; secret material is not cloneable.
use super::{Error, KeyRole, Result};
use pq_signatures::{ml_dsa_87 as ml, SensitiveBytes32};
use rand_core::{OsRng, RngCore};
use zeroize::Zeroizing;

pub mod files;

pub const SUITE: u16 = 87;
pub const PUBLIC_KEY_BYTES: usize = ml::PUBLICKEYBYTES;
pub const SIGNATURE_BYTES: usize = ml::SIGNBYTES;
const HEADER_BYTES: usize = 13;
pub const SECRET_FILE_BYTES: usize = HEADER_BYTES + ml::KEYPAIRBYTES;
const MAGIC: &[u8; 8] = b"RINPQKEY";
const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
	Transfer,
	Proposal,
	Prevote,
	Precommit,
	ValidatorProof,
	PeerAuth,
}
impl Domain {
	fn role(self) -> KeyRole {
		match self {
			Self::Transfer => KeyRole::Transaction,
			Self::PeerAuth => KeyRole::Network,
			_ => KeyRole::Validator,
		}
	}
	fn context(self) -> &'static [u8] {
		match self {
			Self::Transfer => b"RINPQC/M2/TRANSFER",
			Self::Proposal => b"RINPQC/M2/PROPOSAL",
			Self::Prevote => b"RINPQC/M2/PREVOTE",
			Self::Precommit => b"RINPQC/M2/PRECOMMIT",
			Self::ValidatorProof => b"RINPQC/M2/VALIDATOR-PROOF",
			Self::PeerAuth => b"RINPQC/M2/PEER-AUTH",
		}
	}
}

/// Deliberately neither Clone, Debug nor serializable through serde.
pub struct SecretKey {
	key: ml::Keypair,
	role: KeyRole,
}
impl SecretKey {
	pub fn generate(role: KeyRole) -> Result<Self> {
		Self::generate_with(role, os_entropy)
	}
	fn generate_with(role: KeyRole, fill: impl FnOnce(&mut [u8]) -> Result<()>) -> Result<Self> {
		let mut seed = SensitiveBytes32::zeroed();
		fill(seed.as_mut_bytes())?;
		Ok(Self { key: ml::Keypair::generate(&mut seed), role })
	}
	pub fn role(&self) -> KeyRole {
		self.role
	}
	pub fn public_key(&self) -> PublicKey {
		PublicKey { key: self.key.public().clone(), role: self.role }
	}
	pub fn sign(&self, domain: Domain, message: &[u8]) -> Result<Signature> {
		self.sign_with(domain, message, os_entropy)
	}
	fn sign_with(
		&self,
		domain: Domain,
		message: &[u8],
		fill: impl FnOnce(&mut [u8]) -> Result<()>,
	) -> Result<Signature> {
		if domain.role() != self.role {
			return Err(Error::Role);
		}
		if message.len() > ml::MAX_MESSAGE_SIZE {
			return Err(Error::Encoding);
		}
		let mut hedge = SensitiveBytes32::zeroed();
		fill(hedge.as_mut_bytes())?;
		let bytes = self
			.key
			.sign(message, Some(domain.context()), Some(&hedge))
			.map_err(|_| Error::Signature)?;
		Ok(Signature(bytes))
	}
	/// Plaintext secret export. The returned buffer wipes itself on drop; protect any persistent
	/// copy.
	pub fn export(&self) -> Zeroizing<Vec<u8>> {
		let mut bytes = Zeroizing::new(Vec::with_capacity(SECRET_FILE_BYTES));
		bytes.extend_from_slice(MAGIC);
		bytes.extend_from_slice(&VERSION.to_be_bytes());
		bytes.extend_from_slice(&SUITE.to_be_bytes());
		bytes.push(self.role as u8);
		bytes.extend_from_slice(self.key.to_bytes().as_ref());
		bytes
	}
	/// Input ownership stays with the caller, who must wipe its buffer after import.
	pub fn import(bytes: &[u8], expected_role: KeyRole) -> Result<Self> {
		if bytes.len() != SECRET_FILE_BYTES || &bytes[..8] != MAGIC {
			return Err(Error::Encoding);
		}
		if bytes[8..10] != VERSION.to_be_bytes() {
			return Err(Error::Version);
		}
		if bytes[10..12] != SUITE.to_be_bytes() {
			return Err(Error::Suite);
		}
		let role = KeyRole::try_from(bytes[12])?;
		if role != expected_role {
			return Err(Error::Role);
		}
		let key = ml::Keypair::from_bytes(&bytes[HEADER_BYTES..]).map_err(|_| Error::Key)?;
		Ok(Self { key, role })
	}
}
fn os_entropy(bytes: &mut [u8]) -> Result<()> {
	OsRng.try_fill_bytes(bytes).map_err(|_| Error::Config("OS randomness unavailable"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
	key: ml::PublicKey,
	role: KeyRole,
}
impl PublicKey {
	pub fn role(&self) -> KeyRole {
		self.role
	}
	pub fn from_bytes(suite: u16, role: KeyRole, bytes: &[u8]) -> Result<Self> {
		if suite != SUITE {
			return Err(Error::Suite);
		}
		Ok(Self { key: ml::PublicKey::from_bytes(bytes).map_err(|_| Error::Key)?, role })
	}
	pub fn to_bytes(&self) -> [u8; PUBLIC_KEY_BYTES] {
		self.key.to_bytes()
	}
	pub fn verify(&self, domain: Domain, message: &[u8], signature: &Signature) -> Result<()> {
		if self.role != domain.role() {
			return Err(Error::Role);
		}
		if !self.key.verify(message, signature.as_bytes(), Some(domain.context())) {
			return Err(Error::Signature);
		}
		Ok(())
	}
}

/// Public signature bytes, safe to persist and reuse verbatim. Parsing does not authenticate them.
#[derive(Clone, PartialEq, Eq)]
pub struct Signature([u8; SIGNATURE_BYTES]);
impl Signature {
	pub fn from_bytes(suite: u16, bytes: &[u8]) -> Result<Self> {
		if suite != SUITE {
			return Err(Error::Suite);
		}
		let bytes: [u8; SIGNATURE_BYTES] = bytes.try_into().map_err(|_| Error::Signature)?;
		// FIPS 204 hint encoding: cumulative counts, strictly increasing indices per polynomial,
		// zero padding. Full cryptographic validity (including z bounds) is checked by verify.
		const OMEGA: usize = 75;
		const K: usize = 8;
		let hints = &bytes[SIGNATURE_BYTES - OMEGA - K..];
		let mut start = 0;
		for &end in &hints[OMEGA..] {
			let end = usize::from(end);
			if end < start || end > OMEGA || hints[start..end].windows(2).any(|w| w[0] >= w[1]) {
				return Err(Error::Signature);
			}
			start = end;
		}
		if hints[start..OMEGA].iter().any(|&b| b != 0) {
			return Err(Error::Signature);
		}
		Ok(Self(bytes))
	}
	pub fn as_bytes(&self) -> &[u8; SIGNATURE_BYTES] {
		&self.0
	}
}

#[cfg(test)]
mod tests;
