//! Adapter for the pinned Malachite engine. Consensus rules remain in the upstream engine.
pub use malachitebft_app_channel as engine;
mod codec;
mod journal;
mod node;
mod signing;
mod types;
pub use node::{initialize, load_payments, run};

#[cfg(test)]
mod tests;
