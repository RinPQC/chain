//! Component boundaries for the M1 payment node.

pub mod application;
pub mod consensus;
pub mod infrastructure;

pub mod crypto;
pub mod genesis;

pub mod storage;

pub mod mempool;

pub mod rpc;

pub mod payment_cli;

pub mod observability;

/// Explicit M2 wire/storage formats, pending runtime adapter integration.
pub mod m2;
