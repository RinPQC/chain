//! Opt-in subprocess fault injection; absent from normal and release devnet builds.
use super::{
	engine::app::types::{codec::Codec as _, SignedConsensusMsg},
	types::Context,
};

pub fn checkpoint(stage: &str, msg: &SignedConsensusMsg<Context>, codec: &super::codec::Codec) {
	let phase = match msg {
		SignedConsensusMsg::Proposal(_) => "proposal",
		SignedConsensusMsg::Vote(v) if v.message.typ == malachite_types::VoteType::Prevote => {
			"prevote"
		},
		_ => "precommit",
	};
	let value = match msg {
		SignedConsensusMsg::Proposal(p) => Some(p.message.value.0),
		SignedConsensusMsg::Vote(v) => match &v.message.value {
			malachite_types::NilOrVal::Val(value) => Some(value.0),
			malachite_types::NilOrVal::Nil => None,
		},
	};
	// Network precommit cases exercise a lock, rather than a nil vote.
	if phase == "precommit" && value.is_none() {
		return;
	}
	let round =
		std::env::var("RINPQC_FAULT_ROUND").ok().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
	if std::env::var("RINPQC_FAULT").ok().as_deref() == Some(stage)
		&& std::env::var("RINPQC_FAULT_PHASE").ok().as_deref() == Some(phase)
		&& msg.height().0 == 1
		&& msg.round().as_i64() == round
	{
		println!(
			"FAULT stage={stage} phase={phase} height=1 round={round} value={} signed={}",
			value.map(hex::encode).unwrap_or_else(|| "nil".into()),
			hex::encode(codec.encode(msg).expect("fault message encoding"))
		);
		std::process::exit(86);
	}
}
