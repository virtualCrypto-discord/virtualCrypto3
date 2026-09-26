//! Observing behaviour worth warning an operator about, and telling them.
//!
//! The rate limits in [`crate::rate_limit`] are deliberately loose — 120 requests
//! a minute per caller — so they are a courtesy rather than a defence: reaching
//! one is unusual enough to be worth saying out loud, and refusing a request is
//! not the only thing worth doing when a caller keeps being refused elsewhere.
//! What is here watches the refusals this service already makes (an
//! `invalid_token`, a `same_origin` check that failed, a signature Discord would
//! not have signed) and the loose limits besides, counts them per subject inside
//! a window, and warns once per subject per window instead of once per request.
//!
//! It observes and warns; it never refuses. Nothing here changes what a caller
//! is answered — [`crate::state::AppState`]'s limiters do that, and this is
//! beside them rather than in them.
//!
//! The warnings leave two ways: a `tracing` event under the stable target
//! `vc_security` (so `RUST_LOG=vc_security=warn` collects exactly these, and a
//! deployment's log drain can alert on them), and, when
//! `VCRYPTO_SECURITY_WEBHOOK_URL` is configured, a database queue sends reports
//! to the operator's webhook, retrying temporary failures. The log is the record; the webhook is the tap on the
//! shoulder.
//!
//! Counts are in memory and per process, like the limiters beside them. Queued
//! notifications survive restarts; multiple machines share one delivery queue.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

mod webhook;
pub use webhook::WebhookSink;

/// What an operator is being warned about. The name travels in every log field
/// and webhook payload, so it stays stable: a dashboard grepping for
/// `InteractionSignature` should not break because a variant was renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// The loose per-caller limit refused a request (429).
    RateLimited,
    /// The watch's tighter, observation-only window was exceeded — before any
    /// 429, since the loose limit is not the interesting one.
    WatchBurst,
    /// A credential was refused: a token that does not resolve, a session that
    /// is not there. The subject is usually unknown, which is the point.
    AuthFailed,
    /// An authenticated caller asked for something its scopes or operation do
    /// not allow.
    Forbidden,
    /// A cookie-authenticated write failed the `same_origin` check.
    CsrfRejected,
    /// A Discord OAuth callback was refused (no session, a state that does not
    /// match, a code that could not be exchanged).
    CallbackRefused,
    /// An interaction arrived with a signature Discord would not have produced,
    /// or a timestamp outside the accepted skew.
    InteractionSignature,
    /// A webhook handshake was refused by its own rate windows.
    HandshakeRefused,
    /// A registration's webhook answered the handshake wrongly — it does not
    /// verify signatures it is sent.
    WebhookUnverified,
    /// An interaction id was already received. Discord retries, so one is
    /// normal; a flood of them is somebody replaying.
    ReplayDuplicate,
    /// Globally, this window's 5xx rate is abnormal.
    ServerErrorRate,
    /// Globally, this window's request volume is abnormal.
    RequestSurge,
}

impl Signal {
    /// How many occurrences in one window before the first warning is sent.
    ///
    /// Most signals warn on the first occurrence: a `same_origin` failure is
    /// rare enough that one is news. A duplicate interaction receipt is not —
    /// Discord retries on its own, and a receipt still pending answers `409` —
    /// so that one only speaks up when it stops looking like a retry.
    pub fn threshold(self) -> u32 {
        match self {
            Signal::ReplayDuplicate => 10,
            _ => 1,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Signal::RateLimited => "rate_limited",
            Signal::WatchBurst => "watch_burst",
            Signal::AuthFailed => "auth_failed",
            Signal::Forbidden => "forbidden",
            Signal::CsrfRejected => "csrf_rejected",
            Signal::CallbackRefused => "callback_refused",
            Signal::InteractionSignature => "interaction_signature",
            Signal::HandshakeRefused => "handshake_refused",
            Signal::WebhookUnverified => "webhook_unverified",
            Signal::ReplayDuplicate => "replay_duplicate",
            Signal::ServerErrorRate => "server_error_rate",
            Signal::RequestSurge => "request_surge",
        }
    }
}

/// Which of a subject's two possible warnings this is. Two, never more: the
/// first occurrence when the threshold is reached, and one summary at the
/// window's end if anything more happened after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The moment the threshold was reached, sent immediately.
    First,
    /// The window's end, carrying everything that happened after [`Kind::First`].
    Summary,
}

/// One warning, as it leaves: what it is about, and how much of it there was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub signal: Signal,
    pub subject: String,
    pub kind: Kind,
    /// For [`Kind::First`] the count that reached the threshold; for
    /// [`Kind::Summary`] the window's total.
    pub count: u32,
    pub window_secs: u64,
    /// The most recent detail seen in the window.
    pub detail: String,
}

/// A subject's state inside the current window.
struct Entry {
    started: Instant,
    count: u32,
    first_emitted: bool,
    detail: String,
}

impl Entry {
    fn summary(&self, signal: Signal, subject: &str, window: Duration) -> Option<Report> {
        // The first report already covered everything up to the threshold.
        (self.first_emitted && self.count > signal.threshold()).then(|| Report {
            signal,
            subject: subject.to_owned(),
            kind: Kind::Summary,
            count: self.count,
            window_secs: window.as_secs(),
            detail: self.detail.clone(),
        })
    }
}

/// Collect summaries before removing expired entries, whether a request or the
/// timer triggers cleanup. Reports are emitted after the entries lock is released.
fn collect_expired(
    entries: &mut HashMap<(Signal, String), Entry>,
    window: Duration,
    now: Instant,
    reports: &mut Vec<Report>,
) {
    entries.retain(|(signal, subject), entry| {
        if now.duration_since(entry.started) >= window {
            reports.extend(entry.summary(*signal, subject, window));
            false
        } else {
            true
        }
    });
}

/// Requests seen this window, by status class, for the signals no single
/// subject owns: an error rate and a surge are properties of the whole service.
#[derive(Default)]
pub struct RequestCounters {
    total: AtomicU64,
    server_error: AtomicU64,
}

impl RequestCounters {
    pub fn record(&self, status: u16) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if (500..600).contains(&status) {
            self.server_error.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The window's counts, and the counters reset for the next one.
    fn take(&self) -> (u64, u64) {
        (
            self.total.swap(0, Ordering::Relaxed),
            self.server_error.swap(0, Ordering::Relaxed),
        )
    }

    /// What has been counted so far, without resetting: the sweep reads with
    /// [`RequestCounters::take`] and a test reads this the same way — between
    /// sweeps, as the window stands.
    pub fn totals(&self) -> (u64, u64) {
        (
            self.total.load(Ordering::Relaxed),
            self.server_error.load(Ordering::Relaxed),
        )
    }
}

/// 5xx at or above this share of the window — and never fewer than this many —
/// is worth a warning. Below ten, a handful of failures during a deploy is a
/// deploy; above the share, a mostly-broken minute is an incident.
const SERVER_ERROR_MIN: u64 = 10;
const SERVER_ERROR_PERCENT: u64 = 5;

/// Past this many identities, expired watch windows are swept; without it the
/// table would only ever grow, the way [`crate::rate_limit`] bounds its own.
const SWEEP_AT: usize = 10_000;

/// Where a subject's behaviour accumulates, and where warnings come from.
pub struct BehaviorMonitor {
    window: Duration,
    watch_limit: u32,
    watch_window: Duration,
    /// Total requests per minute that count as a surge; zero disables it.
    surge_per_minute: u64,
    entries: Mutex<HashMap<(Signal, String), Entry>>,
    watch: Mutex<HashMap<String, (Instant, u32)>>,
    counters: Arc<RequestCounters>,
    sink: WebhookSink,
}

impl BehaviorMonitor {
    pub fn new(
        window: Duration,
        watch_limit: u32,
        watch_window: Duration,
        surge_per_minute: u64,
    ) -> Self {
        Self {
            window,
            watch_limit,
            watch_window,
            surge_per_minute,
            entries: Mutex::new(HashMap::new()),
            watch: Mutex::new(HashMap::new()),
            counters: Arc::new(RequestCounters::default()),
            sink: WebhookSink::default(),
        }
    }

    pub fn with_webhook(mut self, pool: sqlx::PgPool, url: Option<String>) -> Self {
        self.sink = WebhookSink::new(pool, url);
        self
    }

    /// A monitor with no webhook and short windows, which is what a test that
    /// drives the clock itself wants.
    pub fn for_test() -> Self {
        Self::new(Duration::from_secs(300), 30, Duration::from_secs(10), 3000)
    }

    /// The counters the router's middleware records into. They live here rather
    /// than in the router because the sweep that reads them is this monitor's.
    pub fn counters(&self) -> Arc<RequestCounters> {
        Arc::clone(&self.counters)
    }

    /// Count one occurrence of `signal` for `subject`, warning immediately if it
    /// reaches the signal's threshold and otherwise leaving it to the sweep.
    pub async fn observe(&self, signal: Signal, subject: &str, detail: &str) {
        emit(
            &self.observe_at(signal, subject, detail, Instant::now()),
            &self.sink,
        )
        .await;
    }

    /// [`BehaviorMonitor::observe`], with the moment given: what a test drives
    /// so no test waits on a window.
    fn observe_at(&self, signal: Signal, subject: &str, detail: &str, now: Instant) -> Vec<Report> {
        let mut reports = Vec::new();
        let mut entries = self.entries.lock().expect("the monitor is not poisoned");

        let window = self.window;
        if entries.len() >= SWEEP_AT {
            collect_expired(&mut entries, window, now, &mut reports);
        }

        let entry = entries
            .entry((signal, subject.to_owned()))
            .or_insert(Entry {
                started: now,
                count: 0,
                first_emitted: false,
                detail: String::new(),
            });

        // A request can arrive after expiry but before the next timer sweep.
        // Preserve the old window's summary before starting the new one.
        if now.duration_since(entry.started) >= window {
            reports.extend(entry.summary(signal, subject, window));
            *entry = Entry {
                started: now,
                count: 0,
                first_emitted: false,
                detail: String::new(),
            };
        }

        entry.count = entry.count.saturating_add(1);
        entry.detail = detail.to_owned();

        if !entry.first_emitted && entry.count >= signal.threshold() {
            entry.first_emitted = true;
            reports.push(Report {
                signal,
                subject: subject.to_owned(),
                kind: Kind::First,
                count: entry.count,
                window_secs: window.as_secs(),
                detail: entry.detail.clone(),
            });
        }

        reports
    }

    /// Whether `subject` exceeded the watch's window — an observation-only
    /// count, held separately from and tighter than the loose limit, so a
    /// burst is reported *before* it becomes a 429. Never refuses anything.
    ///
    /// Zero disables it, the way `RATE_LIMIT_PER_MINUTE=0` does.
    pub async fn watch(&self, subject: &str) -> bool {
        let exceeded = self.watch_at(subject, Instant::now());
        if exceeded {
            self.observe(Signal::WatchBurst, subject, "the watch window was exceeded")
                .await;
        }
        exceeded
    }

    fn watch_at(&self, subject: &str, now: Instant) -> bool {
        if self.watch_limit == 0 {
            return false;
        }

        let window = self.watch_window;
        let mut watch = self.watch.lock().expect("the monitor is not poisoned");

        if watch.len() >= SWEEP_AT {
            watch.retain(|_, (started, _)| now.duration_since(*started) < window);
        }

        let entry = watch.entry(subject.to_owned()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }

        entry.1 = entry.1.saturating_add(1);
        entry.1 > self.watch_limit
    }

    /// Every warning the window that just ended owes, and every warning the
    /// window's global counters now justify.
    ///
    /// Called on a timer of one window, which is also what resets the global
    /// counters: an error rate needs a denominator, and the denominator is this
    /// window's requests.
    pub async fn sweep(&self) {
        emit(&self.sweep_at(Instant::now()), &self.sink).await;
    }

    fn sweep_at(&self, now: Instant) -> Vec<Report> {
        let mut reports = Vec::new();

        // The global counters first, while no entries lock is held: observing
        // them takes that lock.
        let (total, server_error) = self.counters.take();
        if total > 0 {
            let share = (total * SERVER_ERROR_PERCENT) / 100;
            if server_error >= share.max(SERVER_ERROR_MIN) {
                let detail = format!("{server_error} of {total} requests failed");
                reports.extend(self.observe_at(
                    Signal::ServerErrorRate,
                    "all-requests",
                    &detail,
                    now,
                ));
            }
        }

        let surge_at = self.surge_per_minute.saturating_mul(self.window.as_secs()) / 60;
        if surge_at > 0 && total > surge_at {
            let detail = format!("{total} requests in one window");
            reports.extend(self.observe_at(Signal::RequestSurge, "all-requests", &detail, now));
        }

        let mut entries = self.entries.lock().expect("the monitor is not poisoned");
        collect_expired(&mut entries, self.window, now, &mut reports);

        reports
    }

    /// What has been counted for a subject in the current window, which is what
    /// a test asserts on rather than on the log.
    pub fn count_for(&self, signal: Signal, subject: &str) -> u32 {
        self.entries
            .lock()
            .expect("the monitor is not poisoned")
            .get(&(signal, subject.to_owned()))
            .map_or(0, |entry| entry.count)
    }

    /// Run the sweep on a timer of one window, forever.
    ///
    /// Its own task rather than the scheduler's: `VCRYPTO_SETTLE_INTERVAL_SECS=0`
    /// turns the contract clock off, and an operator who silenced contract
    /// settlement has not asked for these warnings to stop.
    pub async fn run(self: Arc<Self>) {
        let sweep = async {
            let mut ticker = tokio::time::interval(self.window);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                self.sweep().await;
            }
        };
        tokio::join!(sweep, self.sink.run());
    }
}

/// The warning, once, where an operator can act on it.
async fn emit(reports: &[Report], sink: &WebhookSink) {
    for report in reports {
        tracing::warn!(
        target: "vc_security",
        signal = report.signal.name(),
        subject = %report.subject,
        kind = ?report.kind,
        count = report.count,
        window_secs = report.window_secs,
        detail = %report.detail,
        "suspicious behaviour observed"
        );
    }
    if let Err(error) = sink.enqueue(reports).await {
        tracing::warn!(target: "vc_security", %error, count = reports.len(),
            "the security webhook notifications could not be saved");
    }
}

/// The body the webhook is POSTed, shaped for where it is going.
///
/// Discord and Slack want a line of text under their own keys; anything else —
/// a collector, a router, a service of one's own — gets the fields. The line is
/// the same either way, because what an operator reads first is a sentence.
pub fn payload(url: &str, report: &Report) -> Value {
    let text = format!(
        "[vc_security] {} {} subject={} count={} detail={}",
        report.signal.name(),
        match report.kind {
            Kind::First => "first",
            Kind::Summary => "window summary",
        },
        report.subject,
        report.count,
        report.detail,
    );

    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();

    if host.contains("discord.com") {
        json!({ "content": text })
    } else if host.contains("hooks.slack.com") {
        json!({ "text": text })
    } else {
        json!({
            "signal": report.signal.name(),
            "subject": report.subject,
            "kind": match report.kind {
                Kind::First => "first",
                Kind::Summary => "summary",
            },
            "count": report.count,
            "window_secs": report.window_secs,
            "detail": report.detail,
            "at": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|since| since.as_secs())
                .unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> BehaviorMonitor {
        BehaviorMonitor::new(Duration::from_secs(300), 3, Duration::from_secs(10), 0)
    }

    /// The first occurrence at threshold 1 is a warning of its own, and the
    /// window owes no summary for it: one line told everything there was.
    #[test]
    fn one_occurrence_warns_once_and_never_again_in_the_window() {
        let monitor = monitor();
        let now = Instant::now();

        let first = monitor.observe_at(
            Signal::CsrfRejected,
            "oauth2-approve",
            "invalid_origin",
            now,
        );
        assert_eq!(
            first.first().map(|report| (report.kind, report.count)),
            Some((Kind::First, 1))
        );

        assert!(
            monitor
                .observe_at(
                    Signal::CsrfRejected,
                    "oauth2-approve",
                    "invalid_origin",
                    now
                )
                .is_empty()
        );
        assert_eq!(monitor.count_for(Signal::CsrfRejected, "oauth2-approve"), 2);

        let owed = monitor.sweep_at(now + Duration::from_secs(300));
        assert_eq!(owed.len(), 1, "the window's summary of everything since");
        assert_eq!(owed[0].kind, Kind::Summary);
        assert_eq!(owed[0].count, 2);
        assert_eq!(owed[0].detail, "invalid_origin");
    }

    /// A window that ends with exactly the threshold owes nothing more: the
    /// first warning counted it all.
    #[test]
    fn a_window_that_added_nothing_since_the_first_warning_is_silent() {
        let monitor = monitor();
        let now = Instant::now();

        monitor.observe_at(Signal::AuthFailed, "unauthenticated", "no session", now);

        assert!(
            monitor.sweep_at(now + Duration::from_secs(300)).is_empty(),
            "one occurrence, one line, and no summary to follow it"
        );
    }

    /// The window's end is what starts the subject over.
    #[test]
    fn a_subject_starts_over_in_the_next_window() {
        let monitor = monitor();
        let now = Instant::now();

        monitor.observe_at(Signal::AuthFailed, "unauthenticated", "no session", now);
        monitor.observe_at(Signal::AuthFailed, "unauthenticated", "no session", now);

        let later = now + Duration::from_secs(300);
        let owed = monitor.sweep_at(later);
        assert_eq!(owed.len(), 1);
        assert_eq!(owed[0].count, 2);

        assert_eq!(monitor.count_for(Signal::AuthFailed, "unauthenticated"), 0);

        // And the next occurrence warns as a first again, not as a continuation.
        let first_again = monitor.observe_at(Signal::AuthFailed, "unauthenticated", "why", later);
        assert_eq!(
            first_again.first().map(|report| report.kind),
            Some(Kind::First)
        );
    }

    /// A duplicate receipt is normal once and suspicious as a flood: it does not
    /// speak until it stops looking like Discord's own retry.
    #[test]
    fn a_replay_only_warns_when_it_stops_looking_like_a_retry() {
        let monitor = monitor();
        let now = Instant::now();

        for _ in 0..9 {
            assert!(
                monitor
                    .observe_at(Signal::ReplayDuplicate, "interaction-receipts", "id", now)
                    .is_empty(),
                "nine duplicates are still a retry"
            );
        }

        let tenth = monitor.observe_at(Signal::ReplayDuplicate, "interaction-receipts", "id", now);
        assert_eq!(
            tenth.first().map(|report| (report.kind, report.count)),
            Some((Kind::First, 10))
        );

        assert!(
            monitor
                .observe_at(Signal::ReplayDuplicate, "interaction-receipts", "id", now)
                .is_empty(),
            "the tenth was the warning; the rest wait for the window"
        );

        let owed = monitor.sweep_at(now + Duration::from_secs(300));
        assert_eq!(owed.len(), 1);
        assert_eq!(owed[0].count, 11, "the window's total, retries included");
    }

    /// The watch is its own window, tighter than the loose limit's: it fires on
    /// its own count, and it refuses nothing.
    #[test]
    fn the_watch_fires_on_its_own_count_before_any_limit() {
        let monitor = monitor();
        let now = Instant::now();

        assert!(!monitor.watch_at("account:1", now));
        assert!(!monitor.watch_at("account:1", now));
        assert!(!monitor.watch_at("account:1", now));

        assert!(
            monitor.watch_at("account:1", now),
            "the fourth is over a watch of three"
        );
    }

    /// And a window that has passed starts the watch again.
    #[test]
    fn the_watch_recovers_when_its_window_passes() {
        let monitor = monitor();
        let now = Instant::now();

        for _ in 0..4 {
            monitor.watch_at("account:1", now);
        }

        let later = now + Duration::from_secs(10);
        assert!(!monitor.watch_at("account:1", later));
    }

    /// A watch of zero is no watch, the way `RATE_LIMIT_PER_MINUTE=0` is no
    /// rate limit.
    #[test]
    fn a_watch_of_zero_never_fires() {
        let monitor = BehaviorMonitor::new(Duration::from_secs(300), 0, Duration::from_secs(10), 0);
        let now = Instant::now();

        for _ in 0..100 {
            assert!(!monitor.watch_at("account:1", now));
        }
        assert_eq!(monitor.count_for(Signal::WatchBurst, "account:1"), 0);
    }

    /// The global counters have a denominator: ten failures out of ten requests
    /// is not the same shape as ten out of ten thousand, and the sweep reads
    /// and resets both.
    #[test]
    fn a_bad_error_rate_is_warned_and_the_window_resets() {
        let monitor = monitor();
        let counters = monitor.counters();
        let now = Instant::now();

        for _ in 0..100 {
            counters.record(200);
        }
        for _ in 0..10 {
            counters.record(500);
        }

        let owed = monitor.sweep_at(now);
        assert_eq!(owed.len(), 1, "ten failures in a hundred requests");
        assert_eq!(owed[0].signal, Signal::ServerErrorRate);
        assert_eq!(owed[0].subject, "all-requests");

        // The next window starts from nothing: a rate is a rate *of this
        // window*, and the counters were taken.
        assert!(monitor.sweep_at(now).is_empty());
    }

    /// Below both the share and the floor, a deploy's handful of failures is a
    /// deploy rather than a warning.
    #[test]
    fn a_quiet_error_rate_says_nothing() {
        let monitor = monitor();
        let counters = monitor.counters();
        let now = Instant::now();

        for _ in 0..500 {
            counters.record(200);
        }
        for _ in 0..5 {
            counters.record(500);
        }

        assert!(monitor.sweep_at(now).is_empty());
    }

    /// A surge is total volume against this deployment's own expectation, and
    /// zero disables it.
    #[test]
    fn a_surge_is_measured_against_the_configured_rate() {
        let monitor =
            BehaviorMonitor::new(Duration::from_secs(300), 30, Duration::from_secs(10), 100);
        let counters = monitor.counters();
        let now = Instant::now();

        for _ in 0..501 {
            counters.record(200);
        }

        let owed = monitor.sweep_at(now);
        assert_eq!(owed.len(), 1, "501 requests against 500 in five minutes");
        assert_eq!(owed[0].signal, Signal::RequestSurge);
    }

    /// One subject's counts are another's: a warning about one account says
    /// nothing about the next.
    #[test]
    fn subjects_are_counted_apart() {
        let monitor = monitor();
        let now = Instant::now();

        monitor.observe_at(Signal::Forbidden, "account:1", "Pay", now);
        monitor.observe_at(Signal::Forbidden, "account:1", "Pay", now);

        assert_eq!(monitor.count_for(Signal::Forbidden, "account:1"), 2);
        assert_eq!(monitor.count_for(Signal::Forbidden, "account:2"), 0);
    }

    /// Discord and Slack are sent a line; anything else gets the fields, since
    /// a collector has no use for prose and a human has no use for a table.
    #[test]
    fn the_payload_is_shaped_for_where_it_is_going() {
        let report = Report {
            signal: Signal::AuthFailed,
            subject: "unauthenticated".into(),
            kind: Kind::First,
            count: 1,
            window_secs: 300,
            detail: "no session".into(),
        };

        let discord = payload("https://discord.com/api/webhooks/1/token", &report);
        assert!(discord["content"].as_str().unwrap().contains("auth_failed"));
        assert!(discord.get("signal").is_none(), "Discord wants a line");

        let slack = payload("https://hooks.slack.com/services/T/B/X", &report);
        assert!(slack["text"].as_str().unwrap().contains("unauthenticated"));

        let generic = payload("https://collector.example/hooks", &report);
        assert_eq!(generic["signal"], "auth_failed");
        assert_eq!(generic["subject"], "unauthenticated");
        assert_eq!(generic["count"], 1);
        assert_eq!(generic["detail"], "no session");
        assert!(generic.get("content").is_none(), "a collector wants fields");
    }

    #[test]
    fn a_request_before_the_sweep_preserves_the_previous_windows_summary() {
        let monitor = monitor();
        let now = Instant::now();
        for _ in 0..100 {
            monitor.observe_at(Signal::AuthFailed, "unauthenticated", "old detail", now);
        }

        let later = now + Duration::from_secs(301);
        let reports =
            monitor.observe_at(Signal::AuthFailed, "unauthenticated", "new detail", later);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].kind, Kind::Summary);
        assert_eq!(reports[0].count, 100);
        assert_eq!(reports[0].detail, "old detail");
        assert_eq!(reports[1].kind, Kind::First);
        assert_eq!(reports[1].count, 1);
        assert_eq!(reports[1].detail, "new detail");
        assert!(monitor.sweep_at(later).is_empty(), "no duplicate summary");

        monitor.observe_at(Signal::AuthFailed, "unauthenticated", "new detail", later);
        let reports = monitor.sweep_at(later + Duration::from_secs(300));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].kind, Kind::Summary);
        assert_eq!(reports[0].count, 2, "the new window has its own count");
    }

    #[test]
    fn request_rollover_only_summarizes_occurrences_after_the_first_warning() {
        for (signal, count, owes_summary) in [
            (Signal::AuthFailed, 1, false),
            (Signal::ReplayDuplicate, 9, false),
            (Signal::ReplayDuplicate, 10, false),
            (Signal::ReplayDuplicate, 11, true),
        ] {
            let monitor = monitor();
            let now = Instant::now();
            for _ in 0..count {
                monitor.observe_at(signal, "subject", "old detail", now);
            }
            let later = now + Duration::from_secs(300);
            let reports = monitor.observe_at(signal, "subject", "new detail", later);
            let summaries: Vec<_> = reports
                .iter()
                .filter(|report| report.kind == Kind::Summary)
                .collect();
            assert_eq!(summaries.len(), usize::from(owes_summary));
            if owes_summary {
                assert_eq!(summaries[0].count, count);
                assert_eq!(summaries[0].detail, "old detail");
            }
            assert_eq!(monitor.count_for(signal, "subject"), 1);
            assert!(monitor.sweep_at(later).is_empty());
        }
    }

    #[test]
    fn request_cleanup_preserves_other_subjects_summaries() {
        let monitor = monitor();
        let now = Instant::now();
        for i in 0..SWEEP_AT {
            monitor.observe_at(Signal::AuthFailed, &format!("account:{i}"), "why", now);
        }
        for subject in ["account:0", "account:1"] {
            monitor.observe_at(Signal::AuthFailed, subject, "why", now);
        }

        let later = now + Duration::from_secs(300);
        let reports = monitor.observe_at(Signal::AuthFailed, "new-account", "new", later);
        assert_eq!(reports.len(), 3);
        for subject in ["account:0", "account:1"] {
            assert!(reports.iter().any(|report| report.subject == subject
                && report.kind == Kind::Summary
                && report.count == 2));
            assert_eq!(monitor.count_for(Signal::AuthFailed, subject), 0);
        }
        assert_eq!(reports[2].kind, Kind::First);
        assert_eq!(reports[2].subject, "new-account");
        assert!(
            monitor.sweep_at(later).is_empty(),
            "cleanup already collected the summaries"
        );
    }
}
