//! Domain model and PostgreSQL access for virtualCrypto.

/// Crate version, surfaced in health and telemetry output.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
