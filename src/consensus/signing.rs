//! Domain-separated classical signatures with a durable guard before release to the engine.
use super::{
	engine::app::types::{core::*, SignedConsensusMsg},
	journal::Journal,
	types::{self},
};
use crate::crypto::{public_key, Id, KeyRole, SecretKey};
use async_trait::async_trait;
use ed25519_dalek::Signer as _;
use eyre::{ensure, Result};
use malachite_signing::{Error, Signer, VerificationResult, Verifier};
use std::sync::{
	atomic::{AtomicU64, Ordering},
	Arc,
};

type Ctx = types::Context;
pub struct Signing {
	pub key: SecretKey,
	pub journal: Arc<Journal>,
	pub verifier: Verification,
	/// Next height whose parent has been durably verified. Zero keeps signing disabled.
	pub next_height: Arc<AtomicU64>,
}
#[derive(Clone)]
pub struct Verification {
	pub chain: Id,
}
fn signing_bytes<T: borsh::BorshSerialize>(
	chain: &Id,
	domain: &[u8],
	message: &T,
) -> Result<Vec<u8>> {
	Ok([domain, chain, &borsh::to_vec(message)?].concat())
}
fn vote_bytes(chain: &Id, vote: &types::Vote) -> Result<Vec<u8>> {
	ensure!(
		vote.height.0 > 0
			&& vote.round.as_u32().is_some_and(|r| r <= i32::MAX as u32)
			&& vote.extension.is_none(),
		"invalid vote scope"
	);
	signing_bytes(chain, b"RINPQC/MALACHITE/VOTE/v1\0", vote)
}
fn proposal_bytes(chain: &Id, proposal: &types::Proposal) -> Result<Vec<u8>> {
	ensure!(
		proposal.height.0 > 0
			&& proposal.round.as_u32().is_some_and(|r| r <= i32::MAX as u32)
			&& proposal.pol_round < proposal.round,
		"invalid proposal scope"
	);
	signing_bytes(chain, b"RINPQC/MALACHITE/PROPOSAL/v1\0", proposal)
}
fn verify(bytes: &[u8], signature: &[u8; 64], key: &Id) -> bool {
	public_key(key).is_ok_and(|pk| {
		pk.verify_strict(bytes, &ed25519_dalek::Signature::from_bytes(signature)).is_ok()
	})
}
impl Signing {
	fn raw(&self, bytes: &[u8]) -> Result<[u8; 64]> {
		ensure!(self.key.role() == KeyRole::Validator, "not a validator key");
		Ok(ed25519_dalek::SigningKey::from_bytes(&self.key.seed()).sign(bytes).to_bytes())
	}
	fn guard(&self, msg: &SignedConsensusMsg<Ctx>, phase: u8) -> Result<()> {
		ensure!(
			msg.height().0 == self.next_height.load(Ordering::SeqCst) && msg.height().0 > 0,
			"signing height has no verified durable parent"
		);
		let slot = [
			msg.height().0.to_be_bytes().as_slice(),
			&msg.round().as_u32().ok_or_else(|| eyre::eyre!("nil round"))?.to_be_bytes(),
			&[phase],
		]
		.concat();
		use super::engine::app::types::codec::Codec as _;
		self.journal.signed(&slot, &self.journal.codec.encode(msg)?)
	}
}
#[async_trait]
impl Signer<Ctx> for Signing {
	async fn sign_vote(&self, vote: types::Vote) -> Result<SignedMessage<Ctx, types::Vote>, Error> {
		let result = (|| -> Result<_> {
			ensure!(vote.address.0 == self.key.public_key(), "wrong signing identity");
			let signature = self.raw(&vote_bytes(&self.verifier.chain, &vote)?)?;
			let phase = if vote.typ == VoteType::Prevote { 1 } else { 2 };
			let signed = SignedMessage::new(vote, signature);
			self.guard(&SignedConsensusMsg::Vote(signed.clone()), phase)?;
			Ok(signed)
		})();
		result.map_err(|e| Error::from_source(std::io::Error::other(e.to_string())))
	}
	async fn sign_proposal(
		&self,
		proposal: types::Proposal,
	) -> Result<SignedMessage<Ctx, types::Proposal>, Error> {
		let result = (|| -> Result<_> {
			ensure!(proposal.address.0 == self.key.public_key(), "wrong signing identity");
			let signature = self.raw(&proposal_bytes(&self.verifier.chain, &proposal)?)?;
			let signed = SignedMessage::new(proposal, signature);
			self.guard(&SignedConsensusMsg::Proposal(signed.clone()), 0)?;
			Ok(signed)
		})();
		result.map_err(|e| Error::from_source(std::io::Error::other(e.to_string())))
	}
	async fn sign_vote_extension(
		&self,
		_: VoteExtensionScope<Ctx>,
		_: Vec<u8>,
	) -> Result<SignedMessage<Ctx, Vec<u8>>, Error> {
		Err(Error::new())
	}
	async fn sign_validator_proof(
		&self,
		public: Vec<u8>,
		peer: Vec<u8>,
	) -> Result<ValidatorProof<Ctx>, Error> {
		if public != self.key.public_key() || peer.len() > 128 {
			return Err(Error::new());
		}
		let signature = self
			.raw(&ValidatorProof::<Ctx>::signing_bytes(&public, &peer))
			.map_err(|e| Error::from_source(std::io::Error::other(e.to_string())))?;
		Ok(ValidatorProof::new(public, peer, signature))
	}
}
#[async_trait]
impl Verifier<Ctx> for Verification {
	async fn verify_signed_vote(
		&self,
		vote: &types::Vote,
		sig: &[u8; 64],
		key: &Id,
	) -> Result<VerificationResult, Error> {
		Ok(VerificationResult::from_bool(
			vote.address.0 == *key
				&& vote_bytes(&self.chain, vote).is_ok_and(|b| verify(&b, sig, key)),
		))
	}
	async fn verify_signed_proposal(
		&self,
		proposal: &types::Proposal,
		sig: &[u8; 64],
		key: &Id,
	) -> Result<VerificationResult, Error> {
		Ok(VerificationResult::from_bool(
			proposal.address.0 == *key
				&& proposal_bytes(&self.chain, proposal).is_ok_and(|b| verify(&b, sig, key)),
		))
	}
	async fn verify_signed_vote_extension(
		&self,
		_: &VoteExtensionScope<Ctx>,
		_: &Vec<u8>,
		_: &[u8; 64],
		_: &Id,
	) -> Result<VerificationResult, Error> {
		Ok(VerificationResult::Invalid)
	}
	async fn verify_validator_proof(
		&self,
		proof: &ValidatorProof<Ctx>,
	) -> Result<VerificationResult, Error> {
		let valid = proof.peer_id.len() <= 128
			&& proof
				.public_key
				.as_slice()
				.try_into()
				.is_ok_and(|key| verify(&proof.preimage(), &proof.signature, &key));
		Ok(VerificationResult::from_bool(valid))
	}
}
