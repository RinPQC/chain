use super::{
	codec::Codec,
	engine::app::{
		consensus::Input,
		engine::wal::encode_entry,
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
	Signing { key, journal, verifier: Verification { chain } }
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
		key: SecretKey::from_seed(&[11; 32], KeyRole::Validator),
		journal,
		verifier: Verification { chain },
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
		key: SecretKey::from_seed(&[11; 32], KeyRole::Validator),
		journal,
		verifier: Verification { chain },
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
