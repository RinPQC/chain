//! Adapter for the pinned Malachite engine. Consensus rules remain in the upstream engine.
pub use malachitebft_app_channel as engine;
mod codec;
#[cfg(feature = "fault-injection")]
mod faults;
mod journal;
mod node;
mod signing;
mod sync;
mod types;
pub use node::{initialize, load_payments, run};

#[cfg(test)]
mod tests;
