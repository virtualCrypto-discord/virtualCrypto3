//! Domain model and PostgreSQL access for virtualCrypto.

pub mod balance;
pub mod claim;
pub mod currency;
pub mod db;
pub mod error;
pub mod idempotency;
pub mod issue;
pub mod metadata;
pub mod model;
pub mod payment;
pub mod transfer;
pub mod user;

pub use error::{Error, Result};

/// Crate version, surfaced in health and telemetry output.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
