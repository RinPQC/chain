use super::{
	codec::Codec,
	engine::app::{
		consensus::Input,
		engine::wal::{encode_entry, log_entries},
		types::{
			codec::Codec as _,
			core::{self, Context as _, NilOrVal, Round},
			SignedConsensusMsg,
		},
		wal,
	},
	journal::Journal,
	node::{recover, valid_part},
	signing::{Signing, Verification},
	types::*,
};
use crate::{
	application::execution::Ledger,
	crypto::{KeyRole, SecretKey},
	genesis::Genesis,
	storage::Store,
};
use malachite_signing::{Signer, Verifier, VerifierExt};
use std::sync::Arc;
fn genesis() -> Genesis {
	let f: serde_json::Value =
		serde_json::from_str(include_str!("../../tests/fixtures/m1-identities.json")).unwrap();
	Genesis::from_json(&serde_json::to_vec(&f["genesis"]).unwrap()).unwrap()
}
fn signer(dir: &std::path::Path, seed: u8) -> Signing {
	let key = SecretKey::from_seed(&[seed; 32], KeyRole::Validator);
	let chain = genesis().chain_id().unwrap();
	let journal = Arc::new(
		Journal::create(&dir.join(format!("{seed}.redb")), Codec { chain }, key.public_key())
			.unwrap(),
	);
	Signing {
		wal: Default::default(),
		serial: Default::default(),
		key,
		journal,
		verifier: Verification { chain },
		next_height: Arc::new(std::sync::atomic::AtomicU64::new(1)),
	}
}
fn vote(s: &Signing, round: u32) -> Vote {
	Context.new_prevote(
		Height(1),
		Round::new(round),
		NilOrVal::Val(Value([3; 32])),
		Address(s.key.public_key()),
	)
}
#[tokio::test]
async fn durable_signing_refuses_conflicts_regression_and_cross_chain_replay() {
	let dir = tempfile::tempdir().unwrap();
	let s = signer(dir.path(), 11);
	let v = vote(&s, 1);
	let signed = s.sign_vote(v.clone()).await.unwrap();
	assert_eq!(s.sign_vote(v.clone()).await.unwrap().signature, signed.signature);
	assert!(s
		.verifier
		.verify_signed_vote(&v, &signed.signature, &s.key.public_key())
		.await
		.unwrap()
		.is_valid());
	assert!(Verification { chain: [0; 32] }
		.verify_signed_vote(&v, &signed.signature, &s.key.public_key())
		.await
		.unwrap()
		.is_invalid());
	let mut other_phase = v.clone();
	other_phase.typ = core::VoteType::Precommit;
	assert!(s
		.verifier
		.verify_signed_vote(&other_phase, &signed.signature, &s.key.public_key())
		.await
		.unwrap()
		.is_invalid());
	let chain = s.verifier.chain;
	let public = s.key.public_key();
	drop(s);
	let journal =
		Arc::new(Journal::open(&dir.path().join("11.redb"), Codec { chain }, public).unwrap());
	let s = Signing {
		wal: Default::default(),
		serial: Default::default(),
		key: SecretKey::from_seed(&[11; 32], KeyRole::Validator),
		journal,
		verifier: Verification { chain },
		next_height: Arc::new(std::sync::atomic::AtomicU64::new(1)),
	};
	assert_eq!(s.sign_vote(v.clone()).await.unwrap().signature, signed.signature);
	let mut conflict = v;
	conflict.value = NilOrVal::Nil;
	assert!(s.sign_vote(conflict).await.is_err());
	assert!(s.sign_vote(vote(&s, 2)).await.is_err()); // A failed guard latches the handle closed.
	drop(s);
	let journal =
		Arc::new(Journal::open(&dir.path().join("11.redb"), Codec { chain }, public).unwrap());
	let s = Signing {
		wal: Default::default(),
		serial: Default::default(),
		key: SecretKey::from_seed(&[11; 32], KeyRole::Validator),
		journal,
		verifier: Verification { chain },
		next_height: Arc::new(std::sync::atomic::AtomicU64::new(1)),
	};
	assert!(s.sign_vote(vote(&s, 0)).await.is_err());
}
#[tokio::test]
async fn authenticated_parts_still_require_valid_execution_and_expected_proposer() {
	let dir = tempfile::tempdir().unwrap();
	let g = genesis();
	let validators = Validators::from_genesis(&g);
	let ledger = Ledger::from_genesis(&g).unwrap();
	let expected = Context.select_proposer(&validators, Height(1), Round::ZERO).0;
	let seed = (11..=14)
		.find(|seed| {
			SecretKey::from_seed(&[*seed; 32], KeyRole::Validator).public_key() == expected.0
		})
		.unwrap();
	let s = signer(dir.path(), seed);
	let block = ledger.prepare_block(vec![]).unwrap();
	let proposal = Proposal {
		height: Height(1),
		round: Round::ZERO,
		pol_round: Round::Nil,
		address: expected,
		value: Value(block.block().header.id()),
	};
	let signature = s.sign_proposal(proposal.clone()).await.unwrap().signature;
	let part = Part { proposal, block: block.block().encode().unwrap(), signature };
	assert!(valid_part(&part, &ledger, &validators, &s.verifier).await);
	let mut corrupt = part.clone();
	corrupt.block[110] ^= 1;
	assert!(!valid_part(&corrupt, &ledger, &validators, &s.verifier).await);
	let mut corrupt = part.clone();
	corrupt.proposal.round = Round::new(1);
	assert!(!valid_part(&corrupt, &ledger, &validators, &s.verifier).await);
	let mut corrupt = part.clone();
	corrupt.signature[0] ^= 1;
	assert!(!valid_part(&corrupt, &ledger, &validators, &s.verifier).await);
	let mut corrupt = part;
	corrupt.proposal.pol_round = Round::ZERO;
	assert!(!valid_part(&corrupt, &ledger, &validators, &s.verifier).await);
}
#[tokio::test]
async fn certificates_require_three_distinct_signers_and_recover_commit_before_ack() {
	let dir = tempfile::tempdir().unwrap();
	let g = genesis();
	let validators = Validators::from_genesis(&g);
	let block = Ledger::from_genesis(&g).unwrap().prepare_block(vec![]).unwrap();
	let value = Value(block.block().header.id());
	let mut votes = Vec::new();
	for seed in 11..=13 {
		let s = signer(dir.path(), seed);
		let vote = Context.new_precommit(
			Height(1),
			Round::ZERO,
			NilOrVal::Val(value),
			Address(s.key.public_key()),
		);
		votes.push(s.sign_vote(vote).await.unwrap());
	}
	let cert = core::CommitCertificate::new(Height(1), Round::ZERO, value, votes.clone());
	let verifier = Verification { chain: g.chain_id().unwrap() };
	let check = |c: core::CommitCertificate<Context>| {
		let verifier = verifier.clone();
		let validators = validators.clone();
		async move {
			verifier
				.verify_commit_certificate(
					&Context,
					&c,
					&validators,
					core::ThresholdParams::default(),
				)
				.await
		}
	};
	assert!(check(cert.clone()).await.is_ok());
	let mut short = cert.clone();
	short.commit_signatures.pop();
	assert!(check(short).await.is_err());
	let mut duplicate = cert.clone();
	duplicate.commit_signatures[2] = duplicate.commit_signatures[0].clone();
	assert!(check(duplicate).await.is_err());
	let s = signer(dir.path(), 14);
	s.journal
		.put_once(&[b"b".as_slice(), &value.0].concat(), &block.block().encode().unwrap())
		.unwrap();
	s.journal.save_certificate(&cert).unwrap();
	let path = dir.path().join("application.redb");
	let mut store = Store::create(&path, &g).unwrap();
	recover(&mut store, &s.journal, &verifier, &validators).await.unwrap();
	assert_eq!(store.ledger().unwrap().height(), 1);
	drop(store);
	let mut store = Store::open(&path, &g).unwrap();
	recover(&mut store, &s.journal, &verifier, &validators).await.unwrap();
	assert_eq!(store.ledger().unwrap(), block.candidate_state());
}
#[tokio::test]
async fn signed_message_must_be_present_in_wal_before_restart_can_sign() {
	let dir = tempfile::tempdir().unwrap();
	let s = signer(dir.path(), 11);
	let msg = s.sign_vote(vote(&s, 0)).await.unwrap();
	let path = dir.path().join("engine.wal");
	let mut log = wal::Log::open(&path).unwrap();
	log.reset(1).unwrap();
	log.flush().unwrap();
	drop(log);
	assert!(s.journal.check_wal(&path, 0, s.key.public_key()).is_err());
	let mut log = wal::Log::open(&path).unwrap();
	let mut bytes = Vec::new();
	encode_entry::<Context, _, _>(Input::Vote(msg), &s.journal.codec, &mut bytes).unwrap();
	log.append(bytes).unwrap();
	log.flush().unwrap();
	drop(log);
	s.journal.check_wal(&path, 0, s.key.public_key()).unwrap();
	s.sign_vote(vote(&s, 1)).await.unwrap();
	assert!(s.journal.check_wal(&path, 0, s.key.public_key()).is_err());
	assert!(s.journal.check_wal(&path, 2, s.key.public_key()).is_err());
}
#[tokio::test]
async fn wire_envelopes_reject_wrong_chain_version_trailing_and_oversized_input() {
	let dir = tempfile::tempdir().unwrap();
	let s = signer(dir.path(), 11);
	let signed = SignedConsensusMsg::Vote(s.sign_vote(vote(&s, 0)).await.unwrap());
	let raw = s.journal.codec.encode(&signed).unwrap();
	let decoded: SignedConsensusMsg<Context> = s.journal.codec.decode(raw.clone()).unwrap();
	assert_eq!(decoded, signed);
	let decode = |raw: bytes::Bytes| -> std::io::Result<SignedConsensusMsg<Context>> {
		s.journal.codec.decode(raw)
	};
	let mut bad = raw.to_vec();
	bad[11] ^= 1;
	assert!(decode(bad.into()).is_err());
	let mut bad = raw.to_vec();
	bad.push(0);
	assert!(decode(bad.into()).is_err());
	assert!(decode(vec![0; super::codec::MAX_WIRE + 1].into()).is_err());
	let other: std::io::Result<SignedConsensusMsg<Context>> = Codec { chain: [0; 32] }.decode(raw);
	assert!(other.is_err());
}

#[tokio::test]
async fn restream_preserves_foreign_authorship_and_authenticates_own_reproposal() {
	let dir = tempfile::tempdir().unwrap();
	let proposer = signer(dir.path(), 11);
	let relay = signer(dir.path(), 12);
	let block = Ledger::from_genesis(&genesis()).unwrap().prepare_block(vec![]).unwrap();
	let descriptor = Proposal {
		height: Height(1),
		round: Round::ZERO,
		pol_round: Round::Nil,
		address: Address(proposer.key.public_key()),
		value: Value(block.block().header.id()),
	};
	let signature = proposer.sign_proposal(descriptor.clone()).await.unwrap().signature;
	let part =
		Part { proposal: descriptor.clone(), block: block.block().encode().unwrap(), signature };
	relay.journal.save_part(&part).unwrap();
	let forwarded = super::node::restream_part(
		&relay.journal,
		&relay,
		Address(relay.key.public_key()),
		descriptor.clone(),
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(forwarded, part);
	assert!(relay.journal.get(b"watermark").unwrap().is_none());
	let reproposal = Proposal {
		round: Round::new(1),
		pol_round: Round::ZERO,
		address: Address(relay.key.public_key()),
		..descriptor
	};
	let own = super::node::restream_part(
		&relay.journal,
		&relay,
		Address(relay.key.public_key()),
		reproposal,
	)
	.await
	.unwrap()
	.unwrap();
	assert_eq!(own.block, part.block);
	assert!(relay
		.verifier
		.verify_signed_proposal(&own.proposal, &own.signature, &relay.key.public_key())
		.await
		.unwrap()
		.is_valid());
	let mut changed = own.proposal;
	changed.pol_round = Round::Nil;
	assert!(relay
		.verifier
		.verify_signed_proposal(&changed, &own.signature, &relay.key.public_key())
		.await
		.unwrap()
		.is_invalid());
}

#[test]
fn payment_gossip_has_exact_length_and_rejects_retired_streams() {
	use super::engine::app::{
		streaming::StreamContent,
		types::streaming::{StreamId, StreamMessage},
	};
	let codec = Codec { chain: [7; 32] };
	let msg = StreamMessage::new(
		StreamId::new(bytes::Bytes::from_static(b"retry-1")),
		0,
		StreamContent::Data(Gossip::Payment([0; 180])),
	);
	let raw = codec.encode(&msg).unwrap();
	let decoded: StreamMessage<Gossip> = codec.decode(raw.clone()).unwrap();
	assert_eq!(decoded, msg);
	let decode =
		|raw: bytes::Bytes| -> std::io::Result<StreamMessage<Gossip>> { codec.decode(raw) };
	let mut short = raw.to_vec();
	short.pop();
	assert!(decode(short.into()).is_err());
	let mut long = raw.to_vec();
	long.push(0);
	assert!(decode(long.into()).is_err());
	let retired = codec.pack(4, &msg).unwrap();
	assert!(decode(retired).is_err());
	let wrong_chain: std::io::Result<StreamMessage<Gossip>> = Codec { chain: [8; 32] }.decode(raw);
	assert!(wrong_chain.is_err());
}

async fn sync_payload(block: Vec<u8>) -> super::sync::Payload {
	let dir = tempfile::tempdir().unwrap();
	let value = Value(crate::crypto::hash(
		b"RINPQC/BLOCK/v1\0",
		&block[..crate::application::execution::HEADER_LEN],
	));
	let mut votes = Vec::new();
	for seed in 11..=13 {
		let signer = signer(dir.path(), seed);
		votes.push(
			signer
				.sign_vote(Context.new_precommit(
					Height(1),
					Round::ZERO,
					NilOrVal::Val(value),
					Address(signer.key.public_key()),
				))
				.await
				.unwrap(),
		);
	}
	super::sync::Payload {
		block,
		certificate: core::CommitCertificate::new(Height(1), Round::ZERO, value, votes),
	}
}

#[tokio::test]
async fn sync_wire_is_bounded_chain_bound_and_preserves_one_certificate() {
	use super::engine::app::types::codec::HasEncodedLen;
	use malachite_sync as sync;
	let g = genesis();
	let codec = Codec { chain: g.chain_id().unwrap() };
	let ledger = Ledger::from_genesis(&g).unwrap();
	let payload =
		sync_payload(ledger.prepare_block(vec![]).unwrap().block().encode().unwrap()).await;
	let raw = sync::RawDecidedValue::new(
		codec.pack(14, &payload).unwrap(),
		core::ExtendedCommitCertificate::from_commit_certificate_and_extensions(
			payload.certificate.clone(),
			core::VoteExtensions::new(vec![]),
		),
	);
	let response =
		sync::Response::ValueResponse(sync::ValueResponse::new(Height(1), vec![raw.clone()]));
	let encoded = codec.encode(&response).unwrap();
	assert_eq!(codec.encoded_len(&response).unwrap(), encoded.len());
	let decoded: sync::Response<Context> = codec.decode(encoded.clone()).unwrap();
	assert_eq!(decoded, response);
	let other: std::io::Result<sync::Response<Context>> =
		Codec { chain: [0; 32] }.decode(encoded.clone());
	assert!(other.is_err());
	let mut trailing = encoded.to_vec();
	trailing.push(0);
	let bad: std::io::Result<sync::Response<Context>> = codec.decode(trailing.into());
	assert!(bad.is_err());
	let bad: std::io::Result<sync::Response<Context>> =
		codec.decode(vec![0; super::codec::MAX_WIRE + 1].into());
	assert!(bad.is_err());
	assert!(codec
		.encode(&sync::Response::ValueResponse(sync::ValueResponse::new(
			Height(1),
			vec![raw.clone(), raw.clone()]
		)))
		.is_err());
	let mut mismatch = raw;
	mismatch.certificate.value_id = Value([0; 32]);
	assert!(codec
		.encode(&sync::Response::ValueResponse(sync::ValueResponse::new(Height(1), vec![mismatch])))
		.is_err());
	for height in [Height(1), Height(50)] {
		let request = sync::Request::ValueRequest(sync::ValueRequest::new(height..=height));
		let decoded: sync::Request<Context> =
			codec.decode(codec.encode(&request).unwrap()).unwrap();
		assert_eq!(decoded, request);
		let empty = sync::Response::ValueResponse(sync::ValueResponse::new(height, vec![]));
		let decoded: sync::Response<Context> = codec.decode(codec.encode(&empty).unwrap()).unwrap();
		assert_eq!(decoded, empty);
	}
	for range in [Height(0)..=Height(0), Height(1)..=Height(2), Height(u64::MAX)..=Height(u64::MAX)]
	{
		assert!(codec
			.encode(&sync::Request::ValueRequest(sync::ValueRequest::new(range)))
			.is_err());
	}
	let key = super::engine::app::types::Keypair::ed25519_from_bytes([7u8; 32]).unwrap();
	let peer = sync::PeerId::from_bytes(&key.public().to_peer_id().to_bytes()).unwrap();
	let status =
		sync::Status { peer_id: peer, tip_height: Height(0), history_min_height: Height(1) };
	let decoded: sync::Status<Context> = codec.decode(codec.encode(&status).unwrap()).unwrap();
	assert_eq!(decoded, status);
}

#[tokio::test]
async fn sync_rejects_corruption_wrong_chain_bad_quorum_and_invalid_certified_execution() {
	let g = genesis();
	let ledger = Ledger::from_genesis(&g).unwrap();
	let codec = Codec { chain: ledger.chain_id() };
	let verifier = Verification { chain: ledger.chain_id() };
	let validators = Validators::from_genesis(&g);
	let proposer = Context.select_proposer(&validators, Height(1), Round::ZERO).0;
	let payload =
		sync_payload(ledger.prepare_block(vec![]).unwrap().block().encode().unwrap()).await;
	let check = async |bytes: bytes::Bytes| {
		super::sync::verify_payload(
			&codec,
			&ledger,
			&validators,
			&verifier,
			Height(1),
			Round::ZERO,
			proposer,
			&bytes,
		)
		.await
	};
	assert!(check(codec.pack(14, &payload).unwrap()).await.is_ok());
	let mut cases = Vec::new();
	let mut bad = payload.clone();
	bad.certificate.commit_signatures.pop();
	cases.push(bad);
	let mut bad = payload.clone();
	bad.certificate.commit_signatures[2] = bad.certificate.commit_signatures[0].clone();
	cases.push(bad);
	let mut bad = payload.clone();
	bad.certificate.commit_signatures[0].signature[0] ^= 1;
	cases.push(bad);
	let mut bad = payload.clone();
	bad.block[50] ^= 1;
	cases.push(bad); // Wrong height/parent.
	let mut bad = payload.clone();
	bad.block.push(0);
	cases.push(bad);
	let mut bad = payload.clone();
	bad.block[4] ^= 1;
	cases.push(bad); // Wrong block chain.
	let mut bad = payload.clone();
	bad.certificate.height = Height(2);
	cases.push(bad);
	for bad in cases {
		assert!(check(codec.pack(14, &bad).unwrap()).await.is_err());
	}
	assert!(check(Codec { chain: [0; 32] }.pack(14, &payload).unwrap()).await.is_err());
	// Even an otherwise valid quorum certificate cannot authorize an invalid state root.
	let mut block = payload.block.clone();
	block[110] ^= 1;
	let certified_invalid = sync_payload(block).await;
	assert!(check(codec.pack(14, &certified_invalid).unwrap()).await.is_err());
	assert_eq!(ledger.height(), 0);
}

#[tokio::test]
async fn downloaded_payload_is_not_committed_history_or_signing_readiness() {
	use std::sync::atomic::Ordering;
	let dir = tempfile::tempdir().unwrap();
	let g = genesis();
	let mut store = Store::create(&dir.path().join("application.redb"), &g).unwrap();
	let signer = signer(dir.path(), 14);
	signer.next_height.store(0, Ordering::SeqCst);
	assert!(signer.sign_vote(vote(&signer, 0)).await.is_err());
	let payload = sync_payload(
		store.ledger().unwrap().prepare_block(vec![]).unwrap().block().encode().unwrap(),
	)
	.await;
	let validators = Validators::from_genesis(&g);
	let proposer = Context.select_proposer(&validators, Height(1), Round::ZERO).0;
	super::sync::verify_payload(
		&signer.journal.codec,
		store.ledger().unwrap(),
		&validators,
		&signer.verifier,
		Height(1),
		Round::ZERO,
		proposer,
		&signer.journal.codec.pack(14, &payload).unwrap(),
	)
	.await
	.unwrap();
	signer.journal.save_block(payload.certificate.value_id, &payload.block).unwrap();
	assert!(super::sync::serve(&store, &signer.journal, Height(1)..=Height(1)).unwrap().is_empty());
	recover(&mut store, &signer.journal, &signer.verifier, &validators).await.unwrap();
	assert_eq!(store.ledger().unwrap().height(), 0); // Interrupted download must be requested again.
	assert!(signer.sign_vote(vote(&signer, 0)).await.is_err());
	signer.journal.save_certificate(&payload.certificate).unwrap();
	recover(&mut store, &signer.journal, &signer.verifier, &validators).await.unwrap();
	assert_eq!(store.ledger().unwrap().height(), 1);
	assert_eq!(
		super::sync::serve(&store, &signer.journal, Height(1)..=Height(1)).unwrap().len(),
		1
	);
	assert!(super::sync::serve(&store, &signer.journal, Height(1)..=Height(2)).unwrap().is_empty());
	assert!(super::sync::serve(&store, &signer.journal, Height(2)..=Height(2)).unwrap().is_empty());
	let mut next = vote(&signer, 0);
	next.height = Height(2);
	assert!(signer.sign_vote(next.clone()).await.is_err());
	// The runtime opens this gate only after recovery, WAL checks, and durable commit.
	signer.next_height.store(2, Ordering::SeqCst);
	signer.sign_vote(next.clone()).await.unwrap();
	let signed = signer.sign_vote(next.clone()).await.unwrap();
	assert_eq!(signed, signer.sign_vote(next).await.unwrap());
	let mut future = vote(&signer, 0);
	future.height = Height(3);
	assert!(signer.sign_vote(future).await.is_err());
}

#[tokio::test]
async fn sync_callback_racing_a_commit_does_not_blame_the_peer() {
	use super::engine::app::engine::host::SyncedValueOutcome;
	let dir = tempfile::tempdir().unwrap();
	let g = genesis();
	let mut store = Store::create(&dir.path().join("application.redb"), &g).unwrap();
	let signer = signer(dir.path(), 14);
	let validators = Validators::from_genesis(&g);
	let proposer = Context.select_proposer(&validators, Height(1), Round::ZERO).0;
	let payload = sync_payload(
		store.ledger().unwrap().prepare_block(vec![]).unwrap().block().encode().unwrap(),
	)
	.await;
	let bytes = signer.journal.codec.pack(14, &payload).unwrap();
	let outcome = super::sync::process(
		&store,
		&signer.journal,
		&validators,
		&signer.verifier,
		Height(1),
		Round::ZERO,
		proposer,
		&bytes,
	)
	.await
	.unwrap();
	assert!(matches!(outcome, SyncedValueOutcome::Verdict(_)));
	assert_eq!(store.ledger().unwrap().height(), 0);
	signer.journal.save_certificate(&payload.certificate).unwrap();
	store.commit_decided(1, &payload.block).unwrap();
	let outcome = super::sync::process(
		&store,
		&signer.journal,
		&validators,
		&signer.verifier,
		Height(1),
		Round::ZERO,
		proposer,
		&bytes,
	)
	.await
	.unwrap();
	assert!(matches!(outcome, SyncedValueOutcome::LocalTransientError));
	let proposer = Context.select_proposer(&validators, Height(2), Round::ZERO).0;
	let outcome = super::sync::process(
		&store,
		&signer.journal,
		&validators,
		&signer.verifier,
		Height(2),
		Round::ZERO,
		proposer,
		b"invalid",
	)
	.await
	.unwrap();
	assert!(matches!(outcome, SyncedValueOutcome::PeerFault));
	assert_eq!(store.ledger().unwrap().height(), 1);
}

fn acceptance_block() -> Vec<u8> {
	let fixture: serde_json::Value =
		serde_json::from_str(include_str!("../../tests/fixtures/m1-payment-block.json")).unwrap();
	hex::decode(fixture["block_hex"].as_str().unwrap()).unwrap()
}

#[tokio::test]
async fn decision_process_exit_recovers_payment_only_with_durable_certificate() {
	for stage in ["cached", "certified", "committed"] {
		let dir = tempfile::tempdir().unwrap();
		let output = std::process::Command::new(std::env::current_exe().unwrap())
			.args(["--exact", "consensus::tests::decision_crash_worker", "--nocapture"])
			.env("RINPQC_DECISION_CRASH_DIR", dir.path())
			.env("RINPQC_DECISION_CRASH_STAGE", stage)
			.output()
			.unwrap();
		assert_eq!(
			output.status.code(),
			Some(84),
			"{stage}: {}",
			String::from_utf8_lossy(&output.stderr)
		);
		let g = genesis();
		let chain = g.chain_id().unwrap();
		let raw = acceptance_block();
		let initial = Ledger::from_genesis(&g).unwrap();
		let validated = initial.validate_block(&raw).unwrap();
		let tx_id = validated.outcomes()[0].effect.tx_id;
		for _ in 0..2 {
			let journal = Journal::open(
				&dir.path().join("14.redb"),
				Codec { chain },
				SecretKey::from_seed(&[14; 32], KeyRole::Validator).public_key(),
			)
			.unwrap();
			let mut store = Store::open(&dir.path().join("application.redb"), &g).unwrap();
			recover(&mut store, &journal, &Verification { chain }, &Validators::from_genesis(&g))
				.await
				.unwrap();
			if stage == "cached" {
				assert_eq!(store.ledger().unwrap(), &initial);
				assert!(store.receipt(&tx_id).unwrap().is_none());
				assert!(store.block(1).unwrap().is_none());
			} else {
				assert_eq!(store.ledger().unwrap(), validated.candidate_state());
				assert_eq!(store.block(1).unwrap(), Some(raw.clone()));
				let receipt = store.receipt(&tx_id).unwrap().unwrap();
				assert_eq!(receipt.outcome, validated.outcomes()[0]);
				assert_eq!(receipt.block_id, validated.block().header.id());
			}
		}
	}
}

#[tokio::test]
async fn decision_crash_worker() {
	let Some(directory) = std::env::var_os("RINPQC_DECISION_CRASH_DIR") else {
		return;
	};
	let dir = std::path::Path::new(&directory);
	let stage = std::env::var("RINPQC_DECISION_CRASH_STAGE").unwrap();
	let g = genesis();
	let raw = acceptance_block();
	let block = Ledger::from_genesis(&g).unwrap().validate_block(&raw).unwrap();
	let value = Value(block.block().header.id());
	let mut votes = Vec::new();
	for seed in 11..=13 {
		let s = signer(dir, seed);
		votes.push(
			s.sign_vote(Context.new_precommit(
				Height(1),
				Round::ZERO,
				NilOrVal::Val(value),
				Address(s.key.public_key()),
			))
			.await
			.unwrap(),
		);
	}
	let cert = core::CommitCertificate::new(Height(1), Round::ZERO, value, votes);
	let s = signer(dir, 14);
	let mut store = Store::create(&dir.join("application.redb"), &g).unwrap();
	s.journal.put_once(&[b"b".as_slice(), &value.0].concat(), &raw).unwrap();
	if stage != "cached" {
		s.journal.save_certificate(&cert).unwrap();
	}
	if stage == "committed" {
		store.commit_decided(1, &raw).unwrap();
	}
	// Skip destructors at each durable boundary; this is not a storage-device power cut.
	std::process::exit(84);
}

fn write_recovery_wal(
	path: &std::path::Path,
	codec: &Codec,
	messages: &[SignedConsensusMsg<Context>],
) {
	let mut log = wal::Log::open(path).unwrap();
	log.reset(1).unwrap();
	for msg in messages {
		let input = match msg {
			SignedConsensusMsg::Vote(v) => Input::Vote(v.clone()),
			SignedConsensusMsg::Proposal(p) => Input::Proposal(p.clone()),
		};
		let mut raw = Vec::new();
		encode_entry::<Context, _, _>(input, codec, &mut raw).unwrap();
		log.append(raw).unwrap();
	}
	log.flush().unwrap();
}

#[tokio::test]
async fn wal_first_recovery_authenticates_and_restores_only_a_monotonic_tail() {
	let source_dir = tempfile::tempdir().unwrap();
	let source = signer(source_dir.path(), 11);
	let first = SignedConsensusMsg::Vote(source.sign_vote(vote(&source, 0)).await.unwrap());
	let second = SignedConsensusMsg::Vote(source.sign_vote(vote(&source, 1)).await.unwrap());
	let dir = tempfile::tempdir().unwrap();
	let target = signer(dir.path(), 11);
	let path = dir.path().join("engine.wal");
	let ledger = Ledger::from_genesis(&genesis()).unwrap();
	let validators = Validators::from_genesis(&genesis());
	write_recovery_wal(&path, &target.journal.codec, std::slice::from_ref(&first));
	// Legacy WAL-only entries are never silently adopted.
	assert!(target
		.journal
		.recover_wal(&path, &ledger, &validators, target.key.public_key())
		.await
		.is_err());
	assert!(target.journal.get(b"watermark").unwrap().is_none());
	target.journal.put_once(b"recovery_policy", b"wal-first-v1").unwrap();
	for messages in [vec![first.clone()], vec![first.clone(), second.clone()]] {
		write_recovery_wal(&path, &target.journal.codec, &messages);
		for _ in 0..2 {
			target
				.journal
				.recover_wal(&path, &ledger, &validators, target.key.public_key())
				.await
				.unwrap();
			target.journal.check_wal(&path, 0, target.key.public_key()).unwrap();
		}
		for msg in messages {
			let slot = super::signing::signing_slot(&msg).unwrap();
			assert_eq!(
				target.journal.get(&[b"s".as_slice(), &slot].concat()).unwrap().unwrap(),
				target.journal.codec.encode(&msg).unwrap()
			);
		}
	}
	// Never synthesize a WAL entry from a signature journal after losing engine history.
	write_recovery_wal(&path, &target.journal.codec, &[first]);
	assert!(target
		.journal
		.recover_wal(&path, &ledger, &validators, target.key.public_key())
		.await
		.is_err());
}

#[tokio::test]
async fn wal_first_recovery_rejects_invalid_conflicting_or_corrupt_inputs_without_repair() {
	let source_dir = tempfile::tempdir().unwrap();
	let source = signer(source_dir.path(), 11);
	let valid = source.sign_vote(vote(&source, 0)).await.unwrap();
	let other_dir = tempfile::tempdir().unwrap();
	let other = signer(other_dir.path(), 11);
	let mut different = vote(&other, 0);
	different.value = NilOrVal::Nil;
	let different = other.sign_vote(different).await.unwrap();
	for case in ["signature", "conflict", "checksum", "height", "missing"] {
		let dir = tempfile::tempdir().unwrap();
		let target = signer(dir.path(), 11);
		target.journal.put_once(b"recovery_policy", b"wal-first-v1").unwrap();
		let path = dir.path().join("engine.wal");
		let mut message = valid.clone();
		if case == "signature" {
			message.signature[0] ^= 1;
		}
		if case == "height" {
			message.message.height = Height(2);
		}
		let mut messages = vec![SignedConsensusMsg::Vote(message)];
		if case == "conflict" {
			messages.push(SignedConsensusMsg::Vote(different.clone()));
		}
		write_recovery_wal(&path, &target.journal.codec, &messages);
		if case == "checksum" {
			let mut bytes = std::fs::read(&path).unwrap();
			*bytes.last_mut().unwrap() ^= 1;
			std::fs::write(&path, bytes).unwrap();
		}
		if case == "missing" {
			std::fs::remove_file(&path).unwrap();
		}
		assert!(
			target
				.journal
				.recover_wal(
					&path,
					&Ledger::from_genesis(&genesis()).unwrap(),
					&Validators::from_genesis(&genesis()),
					target.key.public_key()
				)
				.await
				.is_err(),
			"{case}"
		);
		assert!(target.journal.get(b"watermark").unwrap().is_none(), "partial repair in {case}");
	}
}

#[tokio::test]
async fn wal_first_proposal_recovery_requires_the_valid_durable_payload() {
	let g = genesis();
	let validators = Validators::from_genesis(&g);
	let address = Context.select_proposer(&validators, Height(1), Round::ZERO).0;
	let seed = (11..=14)
		.find(|seed| {
			SecretKey::from_seed(&[*seed; 32], KeyRole::Validator).public_key() == address.0
		})
		.unwrap();
	let source_dir = tempfile::tempdir().unwrap();
	let source = signer(source_dir.path(), seed);
	let raw = acceptance_block();
	let ledger = Ledger::from_genesis(&g).unwrap();
	let value = Value(ledger.validate_block(&raw).unwrap().block().header.id());
	let signed = source
		.sign_proposal(Proposal {
			height: Height(1),
			round: Round::ZERO,
			pol_round: Round::Nil,
			address,
			value,
		})
		.await
		.unwrap();
	for (payload, corrupt_part) in [
		(None, false),
		(Some(vec![0]), false),
		(Some(raw.clone()), false),
		(Some(raw.clone()), true),
	] {
		let dir = tempfile::tempdir().unwrap();
		let target = signer(dir.path(), seed);
		target.journal.put_once(b"recovery_policy", b"wal-first-v1").unwrap();
		if let Some(bytes) = &payload {
			target.journal.save_block(value, bytes).unwrap();
		}
		let path = dir.path().join("engine.wal");
		write_recovery_wal(
			&path,
			&target.journal.codec,
			&[SignedConsensusMsg::Proposal(signed.clone())],
		);
		if corrupt_part {
			let key = [
				b"p".as_slice(),
				&1u64.to_be_bytes(),
				&0u32.to_be_bytes(),
				&crate::crypto::hash(
					b"RINPQC/PART-DESCRIPTOR/v1\0",
					&borsh::to_vec(&signed.message).unwrap(),
				),
			]
			.concat();
			let part = Part {
				proposal: signed.message.clone(),
				block: vec![0],
				signature: signed.signature,
			};
			target.journal.put_once(&key, &target.journal.codec.encode(&part).unwrap()).unwrap();
		}
		let result =
			target.journal.recover_wal(&path, &ledger, &validators, target.key.public_key()).await;
		if payload == Some(raw.clone()) && !corrupt_part {
			result.unwrap();
			let parts = target.journal.parts(Height(1), Round::ZERO).unwrap();
			assert_eq!(parts.len(), 1);
			assert_eq!(parts[0].block, raw);
			assert_eq!(parts[0].signature, signed.signature);
		} else {
			assert!(result.is_err());
			assert!(target.journal.get(b"watermark").unwrap().is_none());
		}
	}
}

async fn replay_effect(
	effect: malachite_core::Effect<Context>,
	signer: &Signing,
	votes: &mut Vec<Vote>,
) -> eyre::Result<malachite_core::Resume<Context>> {
	use malachite_core::{Effect, Resumable};
	Ok(match effect {
		Effect::VerifySignature(msg, key, r) => {
			let valid = match msg.message {
				malachite_core::ConsensusMsg::Vote(v) => {
					signer.verifier.verify_signed_vote(&v, &msg.signature, &key).await?
				},
				malachite_core::ConsensusMsg::Proposal(p) => {
					signer.verifier.verify_signed_proposal(&p, &msg.signature, &key).await?
				},
			};
			r.resume_with(valid.is_valid())
		},
		Effect::SignVote(v, r) => {
			votes.push(v.clone());
			r.resume_with(signer.sign_vote(v).await?)
		},
		Effect::CancelAllTimeouts(r)
		| Effect::CancelTimeout(_, r)
		| Effect::ScheduleTimeout(_, r)
		| Effect::StartRound(_, _, _, _, r)
		| Effect::PublishConsensusMsg(_, r)
		| Effect::PublishLivenessMsg(_, r)
		| Effect::RepublishVote(_, r)
		| Effect::RepublishRoundCertificate(_, r)
		| Effect::GetValue(_, _, _, r)
		| Effect::RestreamProposal(_, _, _, _, _, r)
		| Effect::WalAppend(_, _, r) => r.resume_with(()),
		other => panic!("unexpected replay-test effect: {other:?}"),
	})
}

async fn replay_input(
	state: &mut malachite_core::State<Context>,
	input: Input<Context>,
	signer: &Signing,
	metrics: &super::engine::app::metrics::Metrics,
	votes: &mut Vec<Vote>,
) {
	let result: eyre::Result<()> = malachite_core::process!(input: input, state: state, metrics: metrics, with: effect => replay_effect(effect, signer, votes).await);
	result.unwrap();
}

#[tokio::test]
async fn replayed_wal_preserves_lock_against_a_conflicting_later_round_proposal() {
	let g = genesis();
	let validators = Validators::from_genesis(&g);
	let seed_for = |index: usize| {
		(11..=14)
			.find(|seed| {
				SecretKey::from_seed(&[*seed; 32], KeyRole::Validator).public_key()
					== g.validators[index]
			})
			.unwrap()
	};
	let dir = tempfile::tempdir().unwrap();
	let signers: Vec<_> = (0..4).map(|i| signer(dir.path(), seed_for(i))).collect();
	let own = &signers[2]; // Neither round-zero nor round-one proposer.
	let a = Value([3; 32]);
	let b = Value([4; 32]);
	let proposal = Proposal {
		height: Height(1),
		round: Round::ZERO,
		pol_round: Round::Nil,
		address: Address(g.validators[0]),
		value: a,
	};
	let signed = signers[0].sign_proposal(proposal.clone()).await.unwrap();
	let mut inputs = vec![
		Input::Proposal(signed),
		Input::ProposedValue(
			malachite_core::ProposedValue {
				height: Height(1),
				round: Round::ZERO,
				valid_round: Round::Nil,
				proposer: proposal.address,
				value: a,
				validity: core::Validity::Valid,
			},
			core::ValueOrigin::Consensus,
		),
	];
	for signer in &signers[..3] {
		let v = Context.new_prevote(
			Height(1),
			Round::ZERO,
			NilOrVal::Val(a),
			Address(signer.key.public_key()),
		);
		inputs.push(Input::Vote(signer.sign_vote(v).await.unwrap()));
	}
	let precommit = own
		.sign_vote(Context.new_precommit(
			Height(1),
			Round::ZERO,
			NilOrVal::Val(a),
			Address(own.key.public_key()),
		))
		.await
		.unwrap();
	inputs.push(Input::Vote(precommit));
	let path = dir.path().join("replay.wal");
	let mut log = wal::Log::open(&path).unwrap();
	log.reset(1).unwrap();
	for input in inputs {
		let mut raw = Vec::new();
		encode_entry::<Context, _, _>(input, &own.journal.codec, &mut raw).unwrap();
		log.append(raw).unwrap();
	}
	log.flush().unwrap();
	drop(log);
	let params = malachite_core::Params {
		address: Address(own.key.public_key()),
		threshold_params: Default::default(),
		value_payload: core::ValuePayload::ProposalAndParts,
		enabled: false,
	};
	let mut state =
		malachite_core::State::new(Context, Height(1), validators.clone(), params, 100, 100);
	let mut votes = Vec::new();
	let metrics = super::engine::app::metrics::Metrics::default();
	replay_input(
		&mut state,
		Input::StartHeight(Height(1), validators.clone(), false, None, Default::default()),
		own,
		&metrics,
		&mut votes,
	)
	.await;
	let mut log = wal::Log::open(&path).unwrap();
	for input in log_entries::<Context, _>(&mut log, &own.journal.codec).unwrap() {
		replay_input(&mut state, input.unwrap(), own, &metrics, &mut votes).await;
	}
	let locked = state.driver.round_state().locked.as_ref().unwrap();
	assert_eq!(locked.value, a);
	assert_eq!(locked.round, Round::ZERO);
	state.params.enabled = true;
	// f+1 authenticated higher-round votes advance the driver without a quorum for B.
	for signer in &signers[..2] {
		let v = Context.new_prevote(
			Height(1),
			Round::new(1),
			NilOrVal::Val(b),
			Address(signer.key.public_key()),
		);
		replay_input(
			&mut state,
			Input::Vote(signer.sign_vote(v).await.unwrap()),
			own,
			&metrics,
			&mut votes,
		)
		.await;
	}
	assert_eq!(state.driver.round(), Round::new(1));
	let p = Proposal {
		height: Height(1),
		round: Round::new(1),
		pol_round: Round::Nil,
		address: Address(g.validators[1]),
		value: b,
	};
	// Use a separate signer object only to construct this adversarial peer proposal: the
	// peer's vote already occupies a later phase, which our local guard would reject.
	let peer_dir = tempfile::tempdir().unwrap();
	let peer = signer(peer_dir.path(), seed_for(1));
	replay_input(
		&mut state,
		Input::Proposal(peer.sign_proposal(p.clone()).await.unwrap()),
		own,
		&metrics,
		&mut votes,
	)
	.await;
	replay_input(
		&mut state,
		Input::ProposedValue(
			malachite_core::ProposedValue {
				height: Height(1),
				round: Round::new(1),
				valid_round: Round::Nil,
				proposer: p.address,
				value: b,
				validity: core::Validity::Valid,
			},
			core::ValueOrigin::Consensus,
		),
		own,
		&metrics,
		&mut votes,
	)
	.await;
	assert_eq!(state.driver.round_state().locked.as_ref().unwrap().value, a);
	assert!(votes.iter().any(|v| v.round == Round::new(1)
		&& v.typ == core::VoteType::Prevote
		&& v.value == NilOrVal::Nil));
	assert!(!votes.iter().any(|v| v.round == Round::new(1) && v.value == NilOrVal::Val(b)));
}

#[tokio::test]
async fn failed_signing_wal_never_releases_or_journals_the_signature() {
	let dir = tempfile::tempdir().unwrap();
	let s = signer(dir.path(), 11);
	s.wal
		.set(Box::new(|_| Box::pin(async { eyre::bail!("injected WAL flush failure") })))
		.ok()
		.unwrap();
	assert!(s.sign_vote(vote(&s, 0)).await.is_err());
	assert!(s.sign_vote(vote(&s, 1)).await.is_err());
	let chain = s.verifier.chain;
	let validator = s.key.public_key();
	drop(s);
	let journal = Journal::open(&dir.path().join("11.redb"), Codec { chain }, validator).unwrap();
	assert!(journal.get(b"watermark").unwrap().is_none());
}
