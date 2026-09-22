//! Domain model and PostgreSQL access for virtualCrypto.

pub mod application;
pub mod balance;
pub mod claim;
pub mod contract;
pub mod currency;
pub mod db;
pub mod delegation;
pub mod error;
pub mod grant;
pub mod idempotency;
pub mod issue;
pub mod job;
pub mod metadata;
pub mod model;
pub mod notification;
pub mod page;
pub mod payment;
pub mod purge;
pub mod transfer;
pub mod user;

pub use error::{Error, Result};

/// Crate version, surfaced in health and telemetry output.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
