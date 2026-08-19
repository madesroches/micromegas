//! DB-backed audience grant store (#1489, AbAC Stage 6a): a whole-table snapshot cache over the
//! `audience_grants` table (migration v7, `rust/ingestion/src/sql_migration.rs`), checked
//! alongside the existing `{prefix}_AUDIENCE_GRANTS` env map by
//! [`crate::policy::AudienceReadPolicy`]/[`crate::policy::AudienceMintPolicy`] -- a selector
//! present in either source grants access, without either side being deep-cloned or merged into
//! a combined map (`current()` hands back the cached grants behind an `Arc`). This is what makes
//! a grant creatable without a service restart -- the env map stays the static/bootstrap layer.
//!
//! Modeled on `db_api_key.rs`'s config/pool conventions, but a single cached value rather than a
//! per-key `moka` cache: the issue is explicit that the whole map is small enough to hold as one
//! snapshot, so `moka`'s eviction/LRU machinery has nothing to do here.

use crate::db_api_key::resolve_u64;
use crate::policy::{AudienceGrants, GrantAxis};
use crate::types::ProviderUnavailable;
use anyhow::{Context, Result, anyhow};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

/// Cache-TTL knob for [`DbAudienceGrantsSource`], read from env with a default.
#[derive(Clone, Copy, Debug)]
pub struct DbAudienceGrantsConfig {
    /// `{prefix}_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, falling back to
    /// `MICROMEGAS_AUDIENCE_GRANT_CACHE_TTL_SECONDS`, default 60 -- mirrors
    /// `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`'s name, default, and prefix-fallback shape.
    pub cache_ttl_secs: u64,
}

impl DbAudienceGrantsConfig {
    /// Resolves the knob as `{prefix}_AUDIENCE_GRANT_CACHE_TTL_SECONDS` first, falling back to
    /// the unprefixed name -- the same `resolve_u64`-based pattern
    /// `DbApiKeyConfig::from_env_with_prefix` (`db_api_key.rs`) already uses for its four knobs,
    /// so this config follows the same prefix contract every other knob at its wiring sites does.
    /// With an empty prefix this is identical to the unprefixed var.
    pub fn from_env_with_prefix(prefix: &str) -> Self {
        Self {
            cache_ttl_secs: resolve_u64(prefix, "AUDIENCE_GRANT_CACHE_TTL_SECONDS", 60),
        }
    }
}

#[derive(Debug)]
struct Snapshot {
    grants: Arc<AudienceGrants>,
    /// Time of the last *successful* load -- never advanced on a failed refresh. Feeds the age
    /// reported in the refresh-failure log/error context ("how stale is the grant view right
    /// now"), independent of `fetched_at`; it is not itself emitted as a metric.
    loaded_at: Instant,
    /// Time of the last refresh *attempt*, successful or not -- gates how often a failing DB is
    /// re-queried once at least one load has succeeded, so a post-first-success outage costs one
    /// query per TTL, not one per request.
    fetched_at: Instant,
}

/// The whole-table snapshot cache described in the module doc comment.
#[derive(Debug)]
pub struct DbAudienceGrantsSource {
    pool: PgPool,
    ttl: Duration,
    snapshot: tokio::sync::RwLock<Option<Snapshot>>,
    /// Process-start baseline every `last_attempt_at` reading is measured from -- captured once
    /// here, not `Instant::now()` at each read, so `last_attempt_at` values are comparable across
    /// calls the same way a wall-clock timestamp would be, but monotonic: unlike
    /// `Utc::now().timestamp()`, a backwards clock step (NTP correction, VM migration, manual
    /// clock set) can never make a later reading smaller than an earlier one.
    start: Instant,
    /// Milliseconds since `start` of the last refresh *attempt*, recorded outside `Snapshot` so
    /// it exists even before any load has ever succeeded -- mirrors `db_api_key.rs`'s
    /// `last_logged_at`, but monotonic rather than a Unix-epoch-seconds value (see [`Self::start`]).
    /// This is what gates cold-start retries: with no `Snapshot` yet, there is nowhere else to
    /// remember "we just tried and failed a moment ago", so without this field every `current()`
    /// call during process startup against a still-coming-up DB would re-query with no throttling
    /// at all, unlike the post-success path which `fetched_at` already gates. Initialized to
    /// `i64::MIN` -- not `0`, which (being only milliseconds after `start`) would be indistinguishable
    /// from a real attempt made moments after construction and would incorrectly throttle the very
    /// first cold-start call -- so the first real attempt's `saturating_sub` against it is always
    /// far past `ttl` regardless of how soon after construction it lands.
    last_attempt_at: AtomicI64,
}

impl DbAudienceGrantsSource {
    /// Builds a source over `pool` (expected to be a [`crate::db_api_key::dedicated_key_store_pool`],
    /// not the caller's lake pool directly), with no snapshot loaded yet -- the first `current()`
    /// call performs the first query.
    pub fn new(pool: PgPool, config: DbAudienceGrantsConfig) -> Self {
        Self {
            pool,
            ttl: Duration::from_secs(config.cache_ttl_secs),
            snapshot: tokio::sync::RwLock::new(None),
            start: Instant::now(),
            last_attempt_at: AtomicI64::new(i64::MIN),
        }
    }

    /// Milliseconds elapsed since `self.start` -- the monotonic clock `last_attempt_at` is
    /// measured against.
    fn elapsed_millis(&self) -> i64 {
        i64::try_from(self.start.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// Queries the whole table and builds an `AudienceGrants` via
    /// [`AudienceGrants::from_rows`]. The one place a malformed row (one that slipped past the
    /// table's own `CHECK` constraints, e.g. via a direct `psql` session) surfaces as a load
    /// failure rather than a silently-inert or silently-unreadable grant.
    async fn fetch(&self) -> Result<AudienceGrants> {
        let rows = sqlx::query("SELECT audience, axis, selector FROM audience_grants")
            .fetch_all(&self.pool)
            .await
            .context("querying audience_grants")?;
        let mut triples = Vec::with_capacity(rows.len());
        for row in rows {
            let audience: String = row.try_get("audience").context("reading audience")?;
            let axis: String = row.try_get("axis").context("reading axis")?;
            let selector: String = row.try_get("selector").context("reading selector")?;
            let axis = match axis.as_str() {
                "read" => GrantAxis::Read,
                "mint" => GrantAxis::Mint,
                other => {
                    return Err(anyhow!(
                        "audience_grants row for {audience:?}/{selector:?} has unrecognized \
                         axis {other:?}"
                    ));
                }
            };
            triples.push((audience, axis, selector));
        }
        AudienceGrants::from_rows(triples)
    }

    /// The synthesized error for a throttled cold-start window (see [`Self::current`]'s doc
    /// comment) -- never a stored, re-handed-out `anyhow::Error` from a prior attempt, since that
    /// type isn't `Clone` and this store deliberately carries none of `db_api_key.rs`'s
    /// moka/`Arc<E>` sharing machinery to make it so.
    ///
    /// `last_attempt_millis` is measured against `self.start` (see that field's doc comment), not
    /// a wall-clock instant, so there is no absolute timestamp left to report here -- the message
    /// reports the throttle window's remaining duration instead, which stays meaningful (and never
    /// goes backwards) regardless of the system clock.
    fn throttled_error(&self, last_attempt_millis: i64) -> anyhow::Error {
        let since_last_attempt = Duration::from_millis(
            self.elapsed_millis()
                .saturating_sub(last_attempt_millis)
                .max(0) as u64,
        );
        let retry_in = self.ttl.saturating_sub(since_last_attempt);
        // Wrapped in `ProviderUnavailable` (not a bare `anyhow!`) so `caller_context`
        // (`flight_sql_service_impl.rs`) downcasts this to `Status::unavailable` rather than
        // `Status::permission_denied` -- a throttled cold start is a store-outage symptom, not an
        // authorization decision.
        ProviderUnavailable(anyhow!(
            "audience grant store unavailable; cold-start retry throttled, retry available in \
             {} seconds",
            retry_in.as_secs()
        ))
        .into()
    }

    /// Returns the current grant snapshot, refreshing it first if stale.
    ///
    /// Both the cold-start path (no `Snapshot` yet) and the post-success path are throttled to at
    /// most one DB query per TTL window: cold-start via `last_attempt_at` (checked-and-set the
    /// same compare-exchange way `db_api_key.rs::maybe_log_error` rate-limits its own log line),
    /// the post-success path via `Snapshot::fetched_at` as before. A `current()` call that lands
    /// inside an already-throttled window returns the last snapshot if one exists; if not, it
    /// returns a freshly synthesized error describing the throttled cold-start state rather than
    /// storing and re-handing-out the prior attempt's `anyhow::Error`. A concurrent caller that
    /// loses the `last_attempt_at` compare-exchange while the very first cold-start attempt is
    /// still in flight re-checks `self.snapshot` once more before giving up: if the winning
    /// caller has since populated it, the loser serves that snapshot instead of failing; only if
    /// it is still empty does it get the synthesized throttled-state error. It never blocks on
    /// another caller's still-in-flight query, and it never skips the query silently with nothing
    /// to show for it.
    ///
    /// `Err` only when there has never been one successful load -- a fresh process whose first
    /// query hits a down DB has no "last good" to serve, so it fails closed like everything else
    /// on this seam, at a rate capped by `last_attempt_at` rather than once per request. Once any
    /// load has succeeded, a later refresh failure is logged + counted
    /// (`imetric!("audience_grant_refresh_error_count", ...)`) and the last good snapshot keeps
    /// serving, unbounded -- this store has no per-item TTL eviction to fall back on the way
    /// `db_api_key.rs`'s cache does, so an outage degrades to staleness for as long as it lasts,
    /// not just one TTL window.
    pub async fn current(&self) -> Result<Arc<AudienceGrants>> {
        // Fast path: an unexpired snapshot needs no refresh attempt at all.
        {
            let guard = self.snapshot.read().await;
            if let Some(snap) = guard.as_ref()
                && snap.fetched_at.elapsed() < self.ttl
            {
                return Ok(Arc::clone(&snap.grants));
            }
        }

        let had_snapshot = self.snapshot.read().await.is_some();

        if had_snapshot {
            // Post-success path: no single-flight/dedup lock -- a plain `SELECT` with no side
            // effect to de-duplicate, gated only by `fetched_at` above. A few concurrent callers
            // landing right at the TTL boundary can each re-run this; strictly simpler and still
            // cheap (the whole point of "the map is small").
            match self.fetch().await {
                Ok(grants) => {
                    let grants = Arc::new(grants);
                    let mut guard = self.snapshot.write().await;
                    *guard = Some(Snapshot {
                        grants: Arc::clone(&grants),
                        loaded_at: Instant::now(),
                        fetched_at: Instant::now(),
                    });
                    Ok(grants)
                }
                Err(err) => {
                    micromegas_tracing::imetric!(
                        "audience_grant_refresh_error_count",
                        "count",
                        1_u64
                    );
                    let mut guard = self.snapshot.write().await;
                    match guard.as_mut() {
                        Some(snap) => {
                            let age_secs = snap.loaded_at.elapsed().as_secs();
                            micromegas_tracing::error!(
                                "audience grant store refresh failed, serving stale snapshot \
                                 (age={age_secs}s): {err:#}"
                            );
                            snap.fetched_at = Instant::now();
                            Ok(Arc::clone(&snap.grants))
                        }
                        // Snapshot vanished between the check above and now -- this store never
                        // clears a snapshot once set, so this is unreachable in practice; treat
                        // it as a cold-start failure rather than panicking. Wrapped in
                        // `ProviderUnavailable` for the same reason `throttled_error` is: a real
                        // DB failure here must surface to `caller_context` as
                        // `Status::unavailable`, not `Status::permission_denied`.
                        None => Err(ProviderUnavailable(err).into()),
                    }
                }
            }
        } else {
            // Cold-start path: `last_attempt_at`'s compare-exchange *is* the dedup mechanism, so
            // only one caller per TTL window fires the query and the rest get the synthesized
            // throttled-state error instead of racing in. Measured against `self.start`
            // (monotonic), not `Utc::now()` -- a backwards wall-clock step can never widen this
            // window the way it could with an epoch-seconds comparison.
            let now = self.elapsed_millis();
            let prev = self.last_attempt_at.load(Ordering::Relaxed);
            let ttl_millis = self.ttl.as_millis().max(1) as i64;
            // `throttled_millis`, when set, is the timestamp that actually explains the
            // remaining wait: on the throttle-hit branch that's `prev` (still accurate, no
            // exchange attempted); on the CAS-loss branch it's the value the failed
            // `compare_exchange` reports as *currently* stored, not the stale `prev` this
            // caller read before attempting the exchange -- using `prev` there would report
            // the wrong (often ancient/sentinel) retry window instead of the window the
            // winning caller just started.
            let throttled_millis = if now.saturating_sub(prev) < ttl_millis {
                Some(prev)
            } else {
                self.last_attempt_at
                    .compare_exchange(prev, now, Ordering::Relaxed, Ordering::Relaxed)
                    .err()
            };
            if let Some(last_attempt_millis) = throttled_millis {
                // The winner of the compare-exchange may have already finished its first query
                // and populated the snapshot by the time we lost the race -- re-check before
                // treating this as a throttled cold start, so concurrent callers during the very
                // first load don't get denied purely from contention on a healthy DB. A cheap
                // re-check, not a full single-flight/wait: if the winner hasn't finished yet,
                // this still falls through to the throttled error below.
                if let Some(snap) = self.snapshot.read().await.as_ref() {
                    return Ok(Arc::clone(&snap.grants));
                }
                return Err(self.throttled_error(last_attempt_millis));
            }

            match self.fetch().await {
                Ok(grants) => {
                    let grants = Arc::new(grants);
                    let mut guard = self.snapshot.write().await;
                    *guard = Some(Snapshot {
                        grants: Arc::clone(&grants),
                        loaded_at: Instant::now(),
                        fetched_at: Instant::now(),
                    });
                    Ok(grants)
                }
                Err(err) => {
                    micromegas_tracing::imetric!(
                        "audience_grant_refresh_error_count",
                        "count",
                        1_u64
                    );
                    micromegas_tracing::error!(
                        "audience grant store cold-start load failed: {err:#}"
                    );
                    // Wrapped in `ProviderUnavailable` for the same reason `throttled_error` is:
                    // a real DB failure here must surface to `caller_context` as
                    // `Status::unavailable`, not `Status::permission_denied`.
                    Err(ProviderUnavailable(err).into())
                }
            }
        }
    }
}
