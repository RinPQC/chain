//! Channel host for a fixed four-validator development network.
use super::{
	codec::{Codec, MAX_WIRE},
	engine::{
		self,
		app::{
			config,
			engine::host::{Next, SyncedValueOutcome},
			streaming::StreamContent,
			types::{
				codec::Codec as _,
				core::{self, Context as _, HeightParams, Round, Validity},
				streaming::{StreamId, StreamMessage},
				Keypair, LocallyProposedValue, ProposedValue,
			},
		},
		AppMsg, ConsensusContext, EngineBuilder, NetworkContext, NetworkIdentity, NetworkMsg,
		RequestContext, WalContext,
	},
	journal::Journal,
	signing::{Signing, Verification},
	types::*,
};
use crate::{
	application::{execution::Ledger, SignedTransfer},
	infrastructure::{write_new, NodeConfig},
	mempool::PaymentQueue,
	storage::Store,
};
use eyre::{ensure, eyre, Result};
use malachite_signing::{Signer, Verifier, VerifierExt};
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

const READY: &[u8] = b"RINPQC-CONSENSUS-v1";
struct Config {
	consensus: config::ConsensusConfig,
	sync: config::ValueSyncConfig,
	moniker: String,
}
impl config::NodeConfig for Config {
	fn moniker(&self) -> &str {
		&self.moniker
	}
	fn consensus(&self) -> &config::ConsensusConfig {
		&self.consensus
	}
	fn consensus_mut(&mut self) -> &mut config::ConsensusConfig {
		&mut self.consensus
	}
	fn value_sync(&self) -> &config::ValueSyncConfig {
		&self.sync
	}
	fn value_sync_mut(&mut self) -> &mut config::ValueSyncConfig {
		&mut self.sync
	}
}
fn multiaddr(address: SocketAddr) -> Result<engine::app::net::Multiaddr> {
	let family = if address.is_ipv4() { "ip4" } else { "ip6" };
	Ok(format!("/{family}/{}/tcp/{}", address.ip(), address.port()).parse()?)
}
/// Explicit initialization; never fills missing recovery files on node startup.
pub fn initialize(config: &NodeConfig) -> Result<()> {
	config.initialize()?;
	let dir = &config.data_dir;
	if dir.join("consensus.ready").exists() {
		ensure!(
			std::fs::read(dir.join("consensus.ready"))? == READY,
			"incompatible node data version"
		);
		for name in ["application.redb", "consensus.redb", "consensus.wal"] {
			ensure!(dir.join(name).is_file(), "missing recovery file {name}");
		}
		return Ok(());
	}
	for name in ["application.redb", "consensus.redb", "consensus.wal"] {
		ensure!(!dir.join(name).exists(), "partial initialization; inspect data before recovery");
	}
	drop(Store::create(&dir.join("application.redb"), &config.genesis)?);
	drop(Journal::create(
		&dir.join("consensus.redb"),
		Codec { chain: config.genesis.chain_id()? },
		config.validator.public_key(),
	)?);
	let mut log = engine::app::wal::Log::open(dir.join("consensus.wal"))?;
	log.flush()?;
	drop(log);
	write_new(&dir.join("consensus.ready"), READY)?;
	Ok(())
}
fn height_params(validators: &Validators, cadence: u64) -> HeightParams<Context> {
	HeightParams::new(
		validators.clone(),
		core::LinearTimeouts::default(),
		Some(Duration::from_millis(cadence)),
	)
}
fn proposed(part: &Part, validity: Validity) -> ProposedValue<Context> {
	ProposedValue {
		height: part.proposal.height,
		round: part.proposal.round,
		valid_round: part.proposal.pol_round,
		proposer: part.proposal.address,
		value: part.proposal.value,
		validity,
	}
}
pub(super) async fn valid_part(
	part: &Part,
	ledger: &Ledger,
	validators: &Validators,
	verifier: &Verification,
) -> bool {
	let p = &part.proposal;
	if p.height.0 != ledger.height().saturating_add(1)
		|| p.round.as_u32().is_none()
		|| p.pol_round >= p.round
	{
		return false;
	}
	if Context.select_proposer(validators, p.height, p.round).0 != p.address {
		return false;
	}
	if !verifier
		.verify_signed_proposal(p, &part.signature, &p.address.0)
		.await
		.is_ok_and(|r| r.is_valid())
	{
		return false;
	}
	ledger.validate_block(&part.block).is_ok_and(|block| block.block().header.id() == p.value.0)
}
async fn publish(channels: &engine::Channels<Context>, part: Part) -> Result<()> {
	let id = StreamId::new(
		[
			part.proposal.height.0.to_be_bytes().as_slice(),
			&part.proposal.round.as_u32().ok_or_else(|| eyre!("nil round"))?.to_be_bytes(),
			&part.proposal.value.0,
		]
		.concat()
		.into(),
	);
	channels
		.network
		.send(NetworkMsg::PublishProposalPart(StreamMessage::new(
			id,
			0,
			StreamContent::Data(Gossip::Proposal(Box::new(part))),
		)))
		.await
		.map_err(|_| eyre!("network stopped"))
}
// Hidden-lock recovery can ask any validator to relay another proposer's original payload.
pub(super) async fn restream_part(
	journal: &Journal,
	signer: &dyn Signer<Context>,
	own: Address,
	proposal: Proposal,
) -> Result<Option<Part>> {
	if let Some(part) = journal
		.parts(proposal.height, proposal.round)?
		.into_iter()
		.find(|part| part.proposal == proposal)
	{
		return Ok(Some(part));
	}
	if proposal.address != own {
		return Ok(None);
	}
	let block = journal.block(proposal.value)?.ok_or_else(|| eyre!("missing locked payload"))?;
	let signed = signer.sign_proposal(proposal.clone()).await?;
	let part = Part { proposal, block, signature: signed.signature };
	journal.save_part(&part)?;
	Ok(Some(part))
}
async fn check_certificate(
	verifier: &Verification,
	cert: &core::CommitCertificate<Context>,
	validators: &Validators,
) -> Result<()> {
	ensure!(cert.commit_signatures.len() <= 4, "oversized certificate");
	verifier
		.verify_commit_certificate(&Context, cert, validators, core::ThresholdParams::default())
		.await
		.map_err(|e| eyre!("invalid commit certificate: {e:?}"))
}
pub(super) async fn recover(
	store: &mut Store,
	journal: &Journal,
	verifier: &Verification,
	validators: &Validators,
) -> Result<()> {
	// Verify every retained certificate before trusting application history as finalized.
	for height in 1..=store.ledger()?.height() {
		let cert =
			journal.certificate(height)?.ok_or_else(|| eyre!("missing committed certificate"))?;
		ensure!(cert.height.0 == height, "certificate height mismatch");
		check_certificate(verifier, &cert, validators).await?;
		let raw = store.block(height)?.ok_or_else(|| eyre!("missing committed block"))?;
		ensure!(
			journal.block(cert.value_id)?.as_deref() == Some(raw.as_slice()),
			"certificate payload mismatch"
		);
		// Canonical header commitment was already re-executed by Store::open.
		ensure!(
			crate::crypto::hash(
				b"RINPQC/BLOCK/v1\0",
				&raw[..crate::application::execution::HEADER_LEN]
			) == cert.value_id.0,
			"certificate value mismatch"
		);
	}
	let height =
		store.ledger()?.height().checked_add(1).ok_or_else(|| eyre!("height exhausted"))?;
	if let Some(cert) = journal.certificate(height)? {
		ensure!(cert.height.0 == height, "certificate height mismatch");
		check_certificate(verifier, &cert, validators).await?;
		let raw = journal.block(cert.value_id)?.ok_or_else(|| eyre!("missing decided payload"))?;
		let validated = store.ledger()?.validate_block(&raw)?;
		ensure!(validated.block().header.id() == cert.value_id.0, "decision payload mismatch");
		store.commit_decided(height, &raw)?;
	}
	Ok(())
}
/// Start a development validator with optional local queue admission.
/// Every proposed block follows the same validation/commit path.
pub async fn run(config: NodeConfig, payments: Vec<SignedTransfer>) -> Result<()> {
	config.check_directory()?;
	let dir = &config.data_dir;
	ensure!(std::fs::read(dir.join("consensus.ready"))? == READY, "run init before start");
	let chain = config.genesis.chain_id()?;
	let codec = Codec { chain };
	let mut store = Store::open(&dir.join("application.redb"), &config.genesis)?;
	let journal = Arc::new(Journal::open(
		&dir.join("consensus.redb"),
		codec.clone(),
		config.validator.public_key(),
	)?);
	let verifier = Verification { chain };
	let validators = Validators::from_genesis(&config.genesis);
	recover(&mut store, &journal, &verifier, &validators).await?;
	journal.check_wal(
		&dir.join("consensus.wal"),
		store.ledger()?.height(),
		config.validator.public_key(),
	)?;
	let address = Address(config.validator.public_key());
	let signer: Arc<dyn Signer<Context>> = Arc::new(Signing {
		key: config.validator,
		journal: journal.clone(),
		verifier: verifier.clone(),
	});
	let mut seed = config.network.seed();
	let keypair = Keypair::ed25519_from_bytes(seed.as_mut())?;
	let peer = keypair.public().to_peer_id();
	let proof = signer.sign_validator_proof(address.0.to_vec(), peer.to_bytes()).await?;
	let identity = NetworkIdentity::new_validator(
		address.to_string(),
		keypair,
		address.to_string(),
		codec.encode(&proof)?,
	);
	let mut engine_config = Config {
		consensus: config::ConsensusConfig::default(),
		sync: config::ValueSyncConfig::default(),
		moniker: address.to_string(),
	};
	engine_config.sync.enabled = false;
	engine_config.consensus.wal_replay_delay = Duration::ZERO;
	engine_config.consensus.p2p.listen_addr = multiaddr(config.listen)?;
	engine_config.consensus.p2p.persistent_peers_only = true;
	engine_config.consensus.p2p.discovery.enabled = false;
	engine_config.consensus.p2p.pubsub_max_size = bytesize::ByteSize::b(MAX_WIRE as u64);
	engine_config.consensus.p2p.rpc_max_size = bytesize::ByteSize::b(MAX_WIRE as u64);
	engine_config.consensus.p2p.protocol_names.consensus =
		format!("/rinpqc/{}/consensus/v1", hex::encode(chain));
	for (address, key) in config.peers {
		let public = libp2p_identity::ed25519::PublicKey::try_from_bytes(&key)?;
		let peer = libp2p_identity::PublicKey::from(public).to_peer_id();
		engine_config
			.consensus
			.p2p
			.persistent_peers
			.push(format!("{}/p2p/{peer}", multiaddr(address)?).parse()?);
	}
	let mut pending = PaymentQueue::default();
	for payment in payments {
		// Verify before recognizing a finalized retry; a forged signature is not a retry.
		crate::crypto::verify_transfer(&payment, &chain)?;
		if store.receipt(&payment.transfer.id())?.is_none() {
			pending.admit(store.ledger()?, &payment.encode())?;
		}
	}
	let (mut channels, mut handle) = EngineBuilder::new(Context, engine_config)
		.with_default_wal(WalContext::new(dir.join("consensus.wal"), codec.clone()))
		.with_default_network(NetworkContext::new(identity, codec))
		.with_no_sync()
		.with_default_consensus(ConsensusContext::new_validator(
			address,
			Box::new(verifier.clone()),
			Box::new(signer.clone()),
		))
		.with_default_request(RequestContext::new(32))
		.build()
		.await?;
	let mut active_round = Round::ZERO;
	let mut gossip_tick = tokio::time::interval(Duration::from_millis(250));
	gossip_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
	let mut gossip_cursor = None;
	let mut gossip_sequence = 0u64;
	let mut ingress_window = std::time::Instant::now();
	let mut ingress_remaining = 32u32;
	let result = async {
		loop {
			let msg = tokio::select! {
				_ = gossip_tick.tick() => {
					if let Some((key, payment)) = pending.next_gossip(gossip_cursor) {
						// A fresh stream ID permits retry after peers connect or previously reject it.
						gossip_sequence = gossip_sequence.checked_add(1).ok_or_else(|| eyre!("gossip counter exhausted"))?;
						let id = StreamId::new([address.0.as_slice(), &gossip_sequence.to_be_bytes()].concat().into());
						let raw = payment.encode().try_into().map_err(|_| eyre!("payment encoding length"))?;
						let msg = NetworkMsg::PublishProposalPart(StreamMessage::new(id, 0, StreamContent::Data(Gossip::Payment(raw))));
						// Congestion must not block consensus; retry this entry on the next tick.
						match channels.network.try_send(msg) {
							Ok(()) => gossip_cursor = Some(key),
							Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {},
							Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return Err(eyre!("network stopped")),
						}
					}
					continue;
				},
				_ = tokio::signal::ctrl_c() => return Ok(()),
				_ = &mut handle.handle => return Err(eyre!("consensus engine stopped")),
				msg = channels.consensus.recv() => msg.ok_or_else(|| eyre!("consensus channel closed"))?,
			};
			match msg {
				AppMsg::ConsensusReady { reply } => {
					let h = store
						.ledger()?
						.height()
						.checked_add(1)
						.ok_or_else(|| eyre!("height exhausted"))?;
					let _ = reply.send((
						Height(h),
						height_params(&validators, config.genesis.target_interval_ms),
					));
				},
				AppMsg::StartedRound { height, round, proposer, reply_value, .. } => {
					active_round = round;
					println!("ROUND height={} round={} proposer={}", height.0, round, proposer);
					let mut values = Vec::new();
					for part in journal.parts(height, round)? {
						if valid_part(&part, store.ledger()?, &validators, &verifier).await {
							values.push(proposed(&part, Validity::Valid));
						}
					}
					let _ = reply_value.send(values);
				},
				AppMsg::GetValue { height, round, reply, .. } => {
					ensure!(height.0 == store.ledger()?.height() + 1, "proposal height mismatch");
					let cached = journal
						.parts(height, round)?
						.into_iter()
						.find(|p| p.proposal.address == address);
					let part = if let Some(part) = cached {
						ensure!(
							valid_part(&part, store.ledger()?, &validators, &verifier).await,
							"invalid persisted local proposal"
						);
						part
					} else {
						let block = pending.propose(store.ledger()?)?;
						let proposal = Proposal {
							height,
							round,
							pol_round: Round::Nil,
							address,
							value: Value(block.block().header.id()),
						};
						let signed = signer.sign_proposal(proposal.clone()).await?;
						let part = Part {
							proposal,
							block: block.block().encode()?,
							signature: signed.signature,
						};
						journal.save_part(&part)?;
						part
					};
					let _ =
						reply.send(LocallyProposedValue::new(height, round, part.proposal.value));
					publish(&channels, part).await?;
				},
				AppMsg::ReceivedProposalPart { part, reply, .. } => {
					let mut value = None;
					if part.sequence == 0 {
						if let StreamContent::Data(Gossip::Payment(raw)) = &part.content {
							if ingress_window.elapsed() >= Duration::from_secs(1) {
								ingress_window = std::time::Instant::now();
								ingress_remaining = 32;
							}
							if ingress_remaining > 0 {
								ingress_remaining -= 1;
								// Peer rejection is local policy, not a fatal engine error.
								let _ = pending.admit(store.ledger()?, raw);
							}
						}
						if let StreamContent::Data(Gossip::Proposal(part)) = part.content {
							if part.proposal.round.as_i64()
								>= active_round.as_i64().saturating_sub(1)
								&& part.proposal.round.as_i64() <= active_round.as_i64() + 1
								&& valid_part(&part, store.ledger()?, &validators, &verifier).await
							{
								let existing =
									journal.parts(part.proposal.height, part.proposal.round)?;
								if existing.len() < 4
									|| existing.iter().any(|p| p.proposal == part.proposal)
								{
									if !existing.iter().any(|p| p.proposal == part.proposal) {
										journal.save_part(&part)?;
									}
									value = Some(proposed(&part, Validity::Valid));
								}
							}
						}
					}
					let _ = reply.send(value);
				},
				AppMsg::RestreamProposal {
					height,
					round,
					valid_round,
					address: proposer,
					value_id,
				} => {
					let descriptor = Proposal {
						height,
						round,
						pol_round: valid_round,
						address: proposer,
						value: value_id,
					};
					if let Some(part) =
						restream_part(&journal, signer.as_ref(), address, descriptor).await?
					{
						publish(&channels, part).await?;
					} else {
						tracing::warn!("No matching authenticated part available for restream");
					}
				},
				AppMsg::Decided { certificate, reply, .. } => {
					check_certificate(&verifier, &certificate, &validators).await?;
					let block = journal
						.block(certificate.value_id)?
						.ok_or_else(|| eyre!("missing decided payload"))?;
					if certificate.height.0 > store.ledger()?.height() {
						ensure!(
							store.ledger()?.validate_block(&block)?.block().header.id()
								== certificate.value_id.0,
							"decision mismatch"
						);
					}
					journal.save_certificate(&certificate)?;
					store.commit_decided(certificate.height.0, &block)?;
					pending.revalidate(store.ledger()?);
					println!(
						"COMMITTED height={} block_id={} state_root={}",
						store.ledger()?.height(),
						hex::encode(store.ledger()?.block_id()),
						hex::encode(store.ledger()?.state_root())
					);
					let _ = reply.send(());
				},
				AppMsg::Finalized { certificate, reply, .. } => {
					ensure!(
						certificate.height.0 == store.ledger()?.height()
							&& certificate.value_id.0 == store.ledger()?.block_id(),
						"finalization before durable commit"
					);
					let h = store
						.ledger()?
						.height()
						.checked_add(1)
						.ok_or_else(|| eyre!("height exhausted"))?;
					active_round = Round::ZERO;
					let _ = reply.send(Next::Start(
						Height(h),
						height_params(&validators, config.genesis.target_interval_ms),
					));
				},
				AppMsg::ExtendVote { reply, .. } => {
					let _ = reply.send(None);
				},
				AppMsg::VerifyVoteExtension { reply, .. } => {
					let _ = reply.send(Err(
						engine::app::consensus::VoteExtensionError::InvalidVoteExtension,
					));
				},
				AppMsg::GetHistoryMinHeight { reply } => {
					let _ = reply.send(Height(1));
				},
				AppMsg::GetDecidedValues { reply, .. } => {
					let _ = reply.send(Vec::new());
				},
				AppMsg::ProcessSyncedValue { reply, .. } => {
					let _ = reply.send(SyncedValueOutcome::PeerFault);
				},
			}
		}
	}
	.await;
	handle.actor.stop(None);
	// The handle can already have been consumed by select when the engine stopped.
	if !handle.handle.is_finished() {
		let _ = tokio::time::timeout(Duration::from_secs(5), &mut handle.handle).await;
	}
	result
}

pub fn load_payments(path: &Path) -> Result<Vec<SignedTransfer>> {
	let raw =
		crate::infrastructure::read_bounded(path, 4096 * crate::application::SIGNED_TRANSFER_LEN)?;
	ensure!(
		raw.len() % crate::application::SIGNED_TRANSFER_LEN == 0,
		"invalid payment batch length"
	);
	raw.chunks_exact(crate::application::SIGNED_TRANSFER_LEN)
		.map(|chunk| SignedTransfer::decode(chunk).map_err(Into::into))
		.collect()
}
