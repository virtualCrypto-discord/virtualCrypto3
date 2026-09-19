//! `VirtualCrypto.Notification`: what an application is told when a claim of one
//! of its users changes, and when a guild answers one of its asks.

use serde_json::Value;

/// `VirtualCrypto.Notification.Behaviour`.
///
/// Elixir's dispatcher fans out to the modules named in its config; here one
/// implementation is injected instead, which is also the seam the tests watch.
pub trait Notifier: Send + Sync {
    /// `notify_claim_update/2`: one call per claimant, carrying every event for
    /// that claimant.
    fn notify_claim_update(&self, claimant_id: i32, events: &[Value]);

    /// A guild decided an application's ask: the application, and the guild that
    /// decided. The application asked with `grant-requests`, and this is the
    /// push half of that — CIBA's ping to the device flow's poll.
    ///
    /// A yes and a taking-back travel the same way: an approval carries the
    /// scopes granted, a revoke what remains (empty when nothing does). The
    /// poll's answer carries no scopes, so the ping is what says what the grant
    /// still lets the device do — and a token already issued stops being able
    /// to do it, because its scopes are read from the grant.
    fn notify_grant_decided(&self, application_id: i64, guild_id: i64);
}

/// Drops every event. A claim transition still completes; nothing is delivered.
///
/// The webhook transport is not implemented — it needs the application side of
/// the domain, which does not exist here yet — so this is what the server runs
/// with. See `docs/known-gaps.md`.
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify_claim_update(&self, _claimant_id: i32, _events: &[Value]) {}

    fn notify_grant_decided(&self, _application_id: i64, _guild_id: i64) {}
}
