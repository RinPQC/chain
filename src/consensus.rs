//! Adapter boundary for the pinned channel-based consensus engine.
//!
//! Keep engine-specific types here; translate them to application types when
//! implementing the consensus adapter. No engine is started by this scaffold.

pub use malachitebft_app_channel as engine;
