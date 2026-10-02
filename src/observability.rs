//! Per-process diagnostic counters; never inputs to consensus or finality.
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct Metrics {
	started: Instant,
	pub recovered_height: u64,
	pub recovery_duration: Duration,
	pub height: u64,
	pub round: i64,
	pub proposer: String,
	pub finalized_total: u64,
	pub rejected_proposals_total: u64,
	pub sync_verified_total: u64,
	pub sync_rejected_total: u64,
}
impl Default for Metrics {
	fn default() -> Self {
		Self {
			started: Instant::now(),
			recovered_height: 0,
			recovery_duration: Duration::ZERO,
			height: 0,
			round: -1,
			proposer: String::new(),
			finalized_total: 0,
			rejected_proposals_total: 0,
			sync_verified_total: 0,
			sync_rejected_total: 0,
		}
	}
}
impl Metrics {
	pub fn recovered(height: u64, elapsed: Duration) -> Self {
		Self { recovered_height: height, recovery_duration: elapsed, ..Default::default() }
	}
	pub fn snapshot(&self, committed_height: u64, queue_depth: usize) -> Value {
		json!({
			"uptime_seconds": self.started.elapsed().as_secs().to_string(),
			"recovered_height": self.recovered_height.to_string(),
			"recovery_milliseconds": self.recovery_duration.as_millis().to_string(),
			"consensus_height": self.height.to_string(), "round": self.round.to_string(),
			"proposer": self.proposer, "committed_height": committed_height.to_string(),
			"finalized_total": self.finalized_total.to_string(), "queue_depth": queue_depth.to_string(),
			"rejected_proposals_total": self.rejected_proposals_total.to_string(),
			"sync_verified_total": self.sync_verified_total.to_string(),
			"sync_rejected_total": self.sync_rejected_total.to_string()
		})
	}
}
