//! `Discord.Api.Cached`: what the wrapper does that the raw API does not.

mod support;

use support::fake;
use vc_api::discord::{CachedDiscord, DiscordApi};

/// Two lookups of the same id that overlap make one call between them, which is
/// what the Elixir cache's per-key transaction does and what keeps a burst from
/// becoming a burst of Discord traffic.
#[tokio::test]
async fn concurrent_misses_on_one_id_make_one_call() {
    let api = fake();
    let cached = CachedDiscord::new(api.clone());

    let (first, second) = tokio::join!(cached.get_user(123), cached.get_user(123));

    assert!(first.expect("the first lookup").is_some());
    assert!(second.expect("the second lookup").is_some());
    assert_eq!(api.user_calls(), 1, "one call between them");
}

/// Once it is cached, a later lookup does not reach Discord at all.
#[tokio::test]
async fn a_cached_lookup_is_answered_from_the_cache() {
    let api = fake();
    let cached = CachedDiscord::new(api.clone());

    cached.get_user(123).await.expect("the first lookup");
    cached.get_user(123).await.expect("the second lookup");

    assert_eq!(api.user_calls(), 1);
}
