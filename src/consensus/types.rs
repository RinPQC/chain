//! M1 types for the pinned engine. No quorum or locking rules are implemented here.
use super::engine::app::types::core;
use crate::{
	crypto::{Id, SecretKey},
	genesis::Genesis,
};
use borsh::{BorshDeserialize, BorshSerialize};
use std::{fmt, sync::Arc};

#[derive(
	Clone,
	Copy,
	Debug,
	Default,
	Eq,
	PartialEq,
	Ord,
	PartialOrd,
	Hash,
	BorshSerialize,
	BorshDeserialize,
)]
pub struct Height(pub u64);
impl fmt::Display for Height {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.fmt(f)
	}
}
impl core::Height for Height {
	const ZERO: Self = Self(0);
	const INITIAL: Self = Self(1);
	fn increment_by(&self, n: u64) -> Self {
		Self(self.0.checked_add(n).expect("consensus height exhausted"))
	}
	fn decrement_by(&self, n: u64) -> Option<Self> {
		self.0.checked_sub(n).map(Self)
	}
	fn as_u64(&self) -> u64 {
		self.0
	}
}
#[derive(
	Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct Address(pub Id);
impl fmt::Display for Address {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&hex::encode(self.0))
	}
}
impl core::Address for Address {}
// Value IDs are exact block-header commitments; the full block travels in one proposal part.
#[derive(
	Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct Value(pub Id);
impl fmt::Display for Value {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&hex::encode(self.0))
	}
}
impl core::Value for Value {
	type Id = Self;
	fn id(&self) -> Self {
		*self
	}
}
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Proposal {
	pub height: Height,
	pub round: core::Round,
	pub value: Value,
	pub pol_round: core::Round,
	pub address: Address,
}
impl core::Proposal<Context> for Proposal {
	fn height(&self) -> Height {
		self.height
	}
	fn round(&self) -> core::Round {
		self.round
	}
	fn value(&self) -> &Value {
		&self.value
	}
	fn take_value(self) -> Value {
		self.value
	}
	fn pol_round(&self) -> core::Round {
		self.pol_round
	}
	fn validator_address(&self) -> &Address {
		&self.address
	}
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct Vote {
	pub height: Height,
	pub round: core::Round,
	pub value: core::NilOrVal<Value>,
	pub address: Address,
	pub typ: core::VoteType,
	pub extension: Option<core::SignedExtension<Context>>,
}
impl core::Vote<Context> for Vote {
	fn height(&self) -> Height {
		self.height
	}
	fn round(&self) -> core::Round {
		self.round
	}
	fn value(&self) -> &core::NilOrVal<Value> {
		&self.value
	}
	fn take_value(self) -> core::NilOrVal<Value> {
		self.value
	}
	fn vote_type(&self) -> core::VoteType {
		self.typ
	}
	fn validator_address(&self) -> &Address {
		&self.address
	}
	fn extension(&self) -> Option<&core::SignedExtension<Context>> {
		self.extension.as_ref()
	}
	fn take_extension(&mut self) -> Option<core::SignedExtension<Context>> {
		self.extension.take()
	}
	fn extend(mut self, extension: core::SignedExtension<Context>) -> Self {
		self.extension = Some(extension);
		self
	}
}
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Part {
	pub proposal: Proposal,
	pub block: Vec<u8>,
	pub signature: [u8; 64],
}
// The pinned channel host forwards opaque application parts. Payments receive no
// ProposedValue response and never enter the consensus voting state machine.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum Gossip {
	Proposal(Box<Part>),
	Payment([u8; crate::application::SIGNED_TRANSFER_LEN]),
}
impl core::ProposalPart<Context> for Gossip {
	fn is_first(&self) -> bool {
		true
	}
	fn is_last(&self) -> bool {
		true
	}
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Validator(pub Address);
impl core::Validator<Context> for Validator {
	fn address(&self) -> &Address {
		&self.0
	}
	fn public_key(&self) -> &Id {
		&self.0 .0
	}
	fn voting_power(&self) -> u64 {
		1
	}
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Validators(pub [Validator; 4]);
impl Validators {
	pub fn from_genesis(g: &Genesis) -> Self {
		Self(g.validators.map(|id| Validator(Address(id))))
	}
}
impl core::ValidatorSet<Context> for Validators {
	fn count(&self) -> usize {
		4
	}
	fn total_voting_power(&self) -> u64 {
		4
	}
	fn get_by_index(&self, index: usize) -> Option<&Validator> {
		self.0.get(index)
	}
	fn get_by_address(&self, address: &Address) -> Option<&Validator> {
		self.0.iter().find(|v| &v.0 == address)
	}
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ed25519;
impl core::SigningScheme for Ed25519 {
	type DecodingError = crate::crypto::Error;
	type Signature = [u8; 64];
	type PublicKey = Id;
	type PrivateKey = Arc<SecretKey>;
	fn decode_signature(raw: &[u8]) -> Result<Self::Signature, Self::DecodingError> {
		raw.try_into().map_err(|_| crate::crypto::Error::Encoding)
	}
	fn encode_signature(sig: &Self::Signature) -> Vec<u8> {
		sig.to_vec()
	}
	fn decode_public_key(raw: &[u8]) -> Result<Id, Self::DecodingError> {
		let id = raw.try_into().map_err(|_| crate::crypto::Error::Encoding)?;
		crate::crypto::public_key(&id)?;
		Ok(id)
	}
	fn encode_public_key(id: &Id) -> Vec<u8> {
		id.to_vec()
	}
}
#[derive(Clone, Debug)]
pub struct Context;
impl core::Context for Context {
	type Address = Address;
	type Height = Height;
	type ProposalPart = Gossip;
	type Proposal = Proposal;
	type Validator = Validator;
	type ValidatorSet = Validators;
	type Timeouts = core::LinearTimeouts;
	type Value = Value;
	type Vote = Vote;
	type Extension = Vec<u8>;
	type SigningScheme = Ed25519;
	fn select_proposer<'a>(
		&self,
		validators: &'a Validators,
		height: Height,
		round: core::Round,
	) -> &'a Validator {
		let index = ((height.0.saturating_sub(1) % 4
			+ u64::from(round.as_u32().expect("non-nil proposer round")) % 4)
			% 4) as usize;
		&validators.0[index]
	}
	fn new_proposal(
		&self,
		height: Height,
		round: core::Round,
		value: Value,
		pol_round: core::Round,
		address: Address,
	) -> Proposal {
		Proposal { height, round, value, pol_round, address }
	}
	fn new_prevote(
		&self,
		height: Height,
		round: core::Round,
		value: core::NilOrVal<Value>,
		address: Address,
	) -> Vote {
		Vote { height, round, value, address, typ: core::VoteType::Prevote, extension: None }
	}
	fn new_precommit(
		&self,
		height: Height,
		round: core::Round,
		value: core::NilOrVal<Value>,
		address: Address,
	) -> Vote {
		Vote { height, round, value, address, typ: core::VoteType::Precommit, extension: None }
	}
}
