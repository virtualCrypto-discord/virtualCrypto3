//! `VirtualCrypto.Notification`: what an application is told when a claim of one
//! of its users changes.

use serde_json::Value;

/// `VirtualCrypto.Notification.Behaviour`.
///
/// Elixir's dispatcher fans out to the modules named in its config; here one
/// implementation is injected instead, which is also the seam the tests watch.
pub trait Notifier: Send + Sync {
    /// `notify_claim_update/2`: one call per claimant, carrying every event for
    /// that claimant.
    fn notify_claim_update(&self, claimant_id: i32, events: &[Value]);
}

/// Drops every event. A claim transition still completes; nothing is delivered.
///
/// The webhook transport is not implemented — it needs the application side of
/// the domain, which does not exist here yet — so this is what the server runs
/// with. See `docs/known-gaps.md`.
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify_claim_update(&self, _claimant_id: i32, _events: &[Value]) {}
}
