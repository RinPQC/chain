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

/// Hold the test's first consensus height until all three processes have gossip peers.
pub async fn startup_barrier(
	network: malachite_runtime::network::NetworkRef<Context>,
) -> eyre::Result<()> {
	let Ok(ready) = std::env::var("RINPQC_FAULT_READY") else {
		return Ok(());
	};
	let release = std::env::var("RINPQC_FAULT_RELEASE")?;
	tokio::time::timeout(std::time::Duration::from_secs(30), async {
		loop {
			let (reply, received) = tokio::sync::oneshot::channel();
			network
				.cast(malachite_runtime::network::Msg::DumpState(reply.into()))
				.map_err(|_| eyre::eyre!("test network unavailable"))?;
			if received.await?.is_some_and(|state| {
				!state.local_node.subscribed_topics.is_empty()
					&& state
						.peers
						.values()
						.filter(|peer| state.local_node.subscribed_topics.is_subset(&peer.topics))
						.count() >= 2
			}) {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
		std::fs::write(ready, b"ready")?;
		while !std::path::Path::new(&release).is_file() {
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
		}
		Ok::<_, eyre::Report>(())
	})
	.await??;
	Ok(())
}
