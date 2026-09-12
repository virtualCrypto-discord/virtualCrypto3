//! Domain model and PostgreSQL access for virtualCrypto.

pub mod currency;
pub mod db;
pub mod error;
pub mod model;
pub mod user;

pub use error::{Error, Result};

/// Crate version, surfaced in health and telemetry output.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
