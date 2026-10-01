//! Bounded history exchange. Peer tips are hints; only verified certificates and execution
//! authorize state advancement through the ordinary durable decision path.
use super::{
	codec::{invalid, Codec},
	engine::app::types::{
		codec::{Codec as EngineCodec, HasEncodedLen},
		core::{self, Context as _, Round},
	},
	journal::Journal,
	node::check_certificate,
	signing::Verification,
	types::*,
};
use crate::{
	application::execution::{Ledger, MIN_BLOCK_LEN},
	storage::Store,
};
use borsh::{BorshDeserialize, BorshSerialize};
use bytes::Bytes;
use eyre::{ensure, eyre, Result};
use malachite_sync as sync;
use std::{io, ops::RangeInclusive};

/// The host callback omits the engine-verified certificate. Keep the same certificate in
/// the application payload so the host can independently verify it before caching any block.
/// The wire response encodes it once and reconstructs the engine's certificate on decode.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub(super) struct Payload {
	pub block: Vec<u8>,
	pub certificate: core::CommitCertificate<Context>,
}
fn valid_height(height: Height) -> bool {
	height.0 > 0 && height.0 < u64::MAX
}
impl Payload {
	fn check_shape(&self) -> io::Result<()> {
		if !valid_height(self.certificate.height)
			|| self.certificate.round.as_u32().is_none_or(|r| r > i32::MAX as u32)
			|| !(3..=4).contains(&self.certificate.commit_signatures.len())
			|| !(MIN_BLOCK_LEN..=1_048_576).contains(&self.block.len())
		{
			return Err(invalid());
		}
		Ok(())
	}
	fn extended(&self) -> core::ExtendedCommitCertificate<Context> {
		core::ExtendedCommitCertificate::from_commit_certificate_and_extensions(
			self.certificate.clone(),
			core::VoteExtensions::new(vec![]),
		)
	}
	fn to_raw(&self, codec: &Codec) -> io::Result<sync::RawDecidedValue<Context>> {
		self.check_shape()?;
		Ok(sync::RawDecidedValue::new(codec.pack(14, self)?, self.extended()))
	}
}
impl EngineCodec<sync::Status<Context>> for Codec {
	type Error = io::Error;
	fn encode(&self, value: &sync::Status<Context>) -> io::Result<Bytes> {
		check_status(value.tip_height, value.history_min_height)?;
		self.pack(11, &(value.peer_id.to_bytes(), value.tip_height, value.history_min_height))
	}
	fn decode(&self, bytes: Bytes) -> io::Result<sync::Status<Context>> {
		if bytes.len() > 128 {
			return Err(invalid());
		}
		let (peer, tip_height, history_min_height): (Vec<u8>, Height, Height) =
			self.unpack(11, &bytes)?;
		check_status(tip_height, history_min_height)?;
		Ok(sync::Status {
			peer_id: sync::PeerId::from_bytes(&peer).map_err(|_| invalid())?,
			tip_height,
			history_min_height,
		})
	}
}
fn check_status(tip: Height, first: Height) -> io::Result<()> {
	if tip.0 == u64::MAX || first.0 == 0 || first.0 > tip.0 + 1 {
		return Err(invalid());
	}
	Ok(())
}
impl EngineCodec<sync::Request<Context>> for Codec {
	type Error = io::Error;
	fn encode(&self, value: &sync::Request<Context>) -> io::Result<Bytes> {
		let sync::Request::ValueRequest(request) = value;
		if request.range.start() != request.range.end() || !valid_height(*request.range.start()) {
			return Err(invalid());
		}
		self.pack(12, request.range.start())
	}
	fn decode(&self, bytes: Bytes) -> io::Result<sync::Request<Context>> {
		let height: Height = self.unpack(12, &bytes)?;
		if !valid_height(height) {
			return Err(invalid());
		}
		Ok(sync::Request::ValueRequest(sync::ValueRequest::new(height..=height)))
	}
}
impl EngineCodec<sync::Response<Context>> for Codec {
	type Error = io::Error;
	fn encode(&self, value: &sync::Response<Context>) -> io::Result<Bytes> {
		let sync::Response::ValueResponse(response) = value;
		if !valid_height(response.start_height) || response.values.len() > 1 {
			return Err(invalid());
		}
		let payload = response
			.values
			.first()
			.map(|raw| -> io::Result<Payload> {
				let payload: Payload = self.unpack(14, &raw.value_bytes)?;
				payload.check_shape()?;
				if payload.certificate.height != response.start_height
					|| payload.extended() != raw.certificate
				{
					return Err(invalid());
				}
				Ok(payload)
			})
			.transpose()?;
		self.pack(13, &(response.start_height, payload))
	}
	fn decode(&self, bytes: Bytes) -> io::Result<sync::Response<Context>> {
		let (start, payload): (Height, Option<Payload>) = self.unpack(13, &bytes)?;
		if !valid_height(start) {
			return Err(invalid());
		}
		let values = if let Some(payload) = payload {
			if payload.certificate.height != start {
				return Err(invalid());
			}
			vec![payload.to_raw(self)?]
		} else {
			vec![]
		};
		Ok(sync::Response::ValueResponse(sync::ValueResponse::new(start, values)))
	}
}
impl HasEncodedLen<sync::Response<Context>> for Codec {
	fn encoded_len(&self, value: &sync::Response<Context>) -> io::Result<usize> {
		Ok(self.encode(value)?.len())
	}
}

pub(super) fn serve(
	store: &Store,
	journal: &Journal,
	range: RangeInclusive<Height>,
) -> Result<Vec<sync::RawDecidedValue<Context>>> {
	let height = *range.start();
	if range.end() != &height || !valid_height(height) || height.0 > store.ledger()?.height() {
		return Ok(vec![]);
	}
	let block = store.block(height.0)?.ok_or_else(|| eyre!("missing committed sync block"))?;
	let certificate = journal
		.certificate(height.0)?
		.ok_or_else(|| eyre!("missing committed sync certificate"))?;
	ensure!(certificate.height == height, "stored sync certificate height mismatch");
	Ok(vec![Payload { block, certificate }.to_raw(&journal.codec)?])
}

pub(super) async fn verify_payload(
	codec: &Codec,
	ledger: &Ledger,
	validators: &Validators,
	verifier: &Verification,
	height: Height,
	round: Round,
	proposer: Address,
	bytes: &[u8],
) -> Result<Payload> {
	let payload: Payload = codec.unpack(14, bytes)?;
	payload.check_shape()?;
	ensure!(
		Some(height.0) == ledger.height().checked_add(1)
			&& payload.certificate.height == height
			&& payload.certificate.round == round,
		"sync height/round mismatch"
	);
	ensure!(
		Context.select_proposer(validators, height, round).0 == proposer,
		"sync proposer mismatch"
	);
	check_certificate(verifier, &payload.certificate, validators).await?;
	let executed = ledger.validate_block(&payload.block)?;
	ensure!(
		executed.block().header.id() == payload.certificate.value_id.0,
		"sync certificate/block mismatch"
	);
	Ok(payload)
}
