//! Versioned, chain-bound envelopes around the pinned engine's Borsh representation.
use super::{
	engine::app::{
		consensus::LivenessMsg,
		types::{
			codec::Codec as EngineCodec,
			core::{CommitCertificate, PolkaCertificate, ValidatorProof},
			streaming::StreamMessage,
			ProposedValue, SignedConsensusMsg,
		},
	},
	types::*,
};
use crate::crypto::Id;
use borsh::{BorshDeserialize, BorshSerialize};
use bytes::Bytes;
use std::io;

pub const MAX_WIRE: usize = 2 * 1024 * 1024;
#[derive(Clone)]
pub struct Codec {
	pub chain: Id,
}
pub(super) fn invalid() -> io::Error {
	io::Error::new(io::ErrorKind::InvalidData, "invalid consensus envelope")
}
impl Codec {
	pub fn pack<T: BorshSerialize>(&self, tag: u8, value: &T) -> io::Result<Bytes> {
		let mut bytes = b"RINPQC-NET\0\x01".to_vec();
		bytes.extend(self.chain);
		bytes.push(tag);
		value.serialize(&mut bytes)?;
		if bytes.len() > MAX_WIRE {
			return Err(invalid());
		}
		Ok(bytes.into())
	}
	pub fn unpack<T: BorshDeserialize>(&self, tag: u8, bytes: &[u8]) -> io::Result<T> {
		let prefix = b"RINPQC-NET\0\x01";
		let offset = prefix.len() + 32;
		if bytes.len() < offset + 1
			|| bytes.len() > MAX_WIRE
			|| &bytes[..prefix.len()] != prefix
			|| bytes[prefix.len()..offset] != self.chain
			|| bytes[offset] != tag
		{
			return Err(invalid());
		}
		borsh::from_slice(&bytes[offset + 1..])
	}
}
macro_rules! codec {
	($type:ty, $tag:expr) => {
		impl EngineCodec<$type> for Codec {
			type Error = io::Error;
			fn encode(&self, value: &$type) -> io::Result<Bytes> {
				self.pack($tag, value)
			}
			fn decode(&self, bytes: Bytes) -> io::Result<$type> {
				self.unpack($tag, &bytes)
			}
		}
	};
}
codec!(Part, 1);
codec!(Gossip, 10);
codec!(SignedConsensusMsg<Context>, 2);
codec!(LivenessMsg<Context>, 3);
// Tag 4 is retired; old proposal-only streams must not decode as application gossip.
codec!(StreamMessage<Gossip>, 9);
codec!(ProposedValue<Context>, 5);
codec!(PolkaCertificate<Context>, 6);
codec!(CommitCertificate<Context>, 7);
impl EngineCodec<ValidatorProof<Context>> for Codec {
	type Error = io::Error;
	fn encode(&self, proof: &ValidatorProof<Context>) -> io::Result<Bytes> {
		self.pack(8, &(proof.public_key.clone(), proof.peer_id.clone(), proof.signature))
	}
	fn decode(&self, bytes: Bytes) -> io::Result<ValidatorProof<Context>> {
		let (public_key, peer_id, signature): (Vec<u8>, Vec<u8>, [u8; 64]) =
			self.unpack(8, &bytes)?;
		if public_key.len() != 32 || peer_id.len() > 128 {
			return Err(invalid());
		}
		Ok(ValidatorProof::new(public_key, peer_id, signature))
	}
}
