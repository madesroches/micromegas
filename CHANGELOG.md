# Changelog

This file documents the historical progress of the Micromegas project. For current focus, please see the main [README.md](./README.md).

## Unreleased

* **Analytics:**
  * Add `OwnershipRewrite`, Query Enforcement Prong A of the AbAC rollout (#1370, Stage 2 of the epic tracked at #1334) — a mandatory `AnalyzerRule` that injects an audience-filtering predicate into every `MaterializedView`-backed query plan, consuming the `ReadScope` Stage 1 (#1369) already threads into `make_session_context` but did not yet enforce. Resolves one audience per process (`Aggregate(GROUP BY process_id, MAX(audience))` over the raw, time-unbounded `processes` partitions, `NULL`s ignored so a stamped row always outranks an unstamped one) rather than filtering raw partition rows directly, closing a fail-open gap the parent plan's own `IN (SELECT process_id FROM processes WHERE ...)` construction would have left (a process's pre-stamping rows would otherwise keep it visible under the escape hatch below forever after a later, narrower stamp). Branches per view set: `processes`' own scan and every process_id-**column** view (`streams`, `blocks`, `log_entries`, `measures`, `net_spans`, `otel_spans`, `images`, `log_stats`) get a `process_id IN (subquery)` semi-join; `async_events` and `thread_spans` — process/stream-scoped but with no `process_id` (or, for `thread_spans`, no `stream_id`) column to join on — get a literal-valued `EXISTS` keyed on `get_view_instance_id()` instead (`thread_spans`' additionally resolves through `streams`); a new `MICROMEGAS_PUBLIC_VIEW_SETS` allowlist (comma-separated, empty/off by default) skips the predicate entirely for named, genuinely aggregated/non-PII view sets; any view set matching none of the above makes `analyze()` fail loudly (`DataFusionError::Plan`) rather than silently plan an unfiltered scan. `ReadScope::All` (the internal/maintenance marker) is a true no-op. A new `MICROMEGAS_UNSTAMPED_AUDIENCE` knob (`{prefix}_UNSTAMPED_AUDIENCE`, falling back to the unprefixed form) names the audience a process with no `micromegas.audience` property coalesces to — unset (default) means unstamped processes stay invisible to every `ReadScope::Audiences` caller. Both knobs are bundled into a new `OwnershipRewriteConfig`, parsed in `micromegas-analytics` (not `micromegas-auth`, mirroring Stage 1's own crate-boundary reasoning) and carried on a new `CallerContext.ownership_config` field rather than a new `make_session_context` parameter. **Upgrade note, read before deploying with auth enabled**: any deployment already running with an `AudienceReadPolicy` active goes from full visibility today (nothing yet filters a `ReadScope::Audiences` session) to **zero visible rows** the instant this ships, unless `MICROMEGAS_UNSTAMPED_AUDIENCE=public` is set in the *same* deploy — `public` is the sole built-in read grant every authenticated principal has under #1372's grant-map model below, so this one knob is enough (that model replaces the identity-derived singleton `{user:<email>}` this note originally had to widen with a second `MICROMEGAS_IMPLICIT_GROUPS` knob; #1372 removes that knob outright, and the relaxed audience charset it ships makes the originally-recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` fail startup, so use `public` instead). Ingestion-time stamping has since landed (Stage 5, #1373), so this knob is now required only for data ingested *before* that stage shipped, or ingested under a credential with no bound audience (env-keyring key, OIDC) — see that entry below for what does and doesn't carry a write audience. An auth-unset deployment (no `AuthContext` extension, e.g. `--disable-auth`) is completely unaffected. **This escape hatch has a further precondition the knob alone doesn't satisfy**: `OwnershipRewrite` resolves a process's audience from the *materialized* `processes`/`streams` views (an `Aggregate` over `__processes__partitions`), and those are `SqlBatchView`s whose `jit_update` is a no-op — they are only ever populated by the maintenance daemon's periodic `materialize_all_views` pass. A process entirely absent from that materialized view contributes nothing to the coalesced `IN`/`EXISTS` check regardless of the env var, so a running, caught-up maintenance role is an additional, undeclared precondition: every process ingested since the daemon's last pass, and *all* processes if the daemon isn't running or deployed, stays invisible to every `ReadScope::Audiences` caller — including a caller's own just-ingested data — until the daemon catches up. **Known limitation, not yet closed**: `micromegas.audience` is read as a plain, client-supplied process property with no server-side validation — any instrumented process can set it to whatever value it likes, including one that hides its own data from every legitimate caller or spoofs another principal's audience. The intended trust anchor is the ingestion API key, not the client payload: each key is assigned exactly one write audience (Stage 4, #1372 below) carried authenticated into `AuthContext.bound_audience`. Stage 5 (#1373, see the **Ingestion** entry below) has since landed and closes this gap — `micromegas.audience` is now stamped server-side from that authenticated value, and a client-supplied `micromegas.*` property is stripped rather than trusted — so this stage's enforcement is now a real security boundary against a malicious or misconfigured instrumented client, not merely a client-asserted label. Nor is it one against an authenticated *reader*: `process_spans`, `perfetto_trace_chunks`, and `parse_block` (five `TODO(#1371)` call sites across `process_spans_table_function.rs`, `perfetto_trace_execution_plan.rs`, `parse_block_table_function.rs`, and `metadata.rs`) each plan their own internal session under `ReadScope::All`, which makes `OwnershipRewrite` a no-op for them — a restricted, authenticated caller can read any process's span/block data through these three table-valued functions today, completely unfiltered by audience, until Prong B (#1371) closes this gap. **Minor breaking change**: `FlightSqlServiceImpl::new` (published API, `micromegas::servers::flight_sql_service_impl`) gains a required `ownership_config: Arc<OwnershipRewriteConfig>` parameter; the public, struct-literal-constructed `CallerContext` (`micromegas_analytics::lakehouse::read_scope`) gains a new public field, `ownership_config`. `FlightSqlServerBuilder` gains `with_ownership_config()`. **Also breaking for a caller-supplied `ViewFactory`**: `make_session_context` now requires the `ViewFactory` (`FlightSqlServerBuilder::with_view_factory_fn`, published API) to register both the `processes` and `streams` global views whenever a request's `ReadScope` is not `ReadScope::All` — `OwnershipRewrite` reads its audience mapping from them — so a custom `ViewFactory` that omits either one now fails every such request at session creation instead of succeeding.
* **Auth:**
  * Give every `ingestion_api_keys` row a single, immutable write audience, carried authenticated into `AuthContext.bound_audience` (#1372, Stage 4 of the epic tracked at #1334) — the value Stage 5 (#1373, see the **Ingestion** entry below) stamps `micromegas.audience` from. **Also settles what an audience *is*, replacing Stage 1's (#1369) prefixed, identity-derived model**: an audience is now an opaque label (`is_valid_audience`: `[A-Za-z0-9_-]{1,255}`, case-sensitive, no normalization) on a bucket of data, and who may read/mint into it is separate, editable configuration — a new `AudienceGrants` map, parsed from JSON at `{prefix}_AUDIENCE_GRANTS`/`MICROMEGAS_AUDIENCE_GRANTS` and keyed by audience name, each value either a bare array (read-only shorthand) or `{"read": [...], "mint": [...]}` for an audience that also grants mint authority; selectors are `*` (anyone authenticated), `user:<email>`, or `group:<g>`, validated at parse time (unknown-shaped key/selector or a duplicate JSON key ⇒ startup `Err`, not a silently-inert entry). `public` is the sole built-in: every authenticated principal reads it, with no config needed. There is **no self-audience rule** (no "you may read the audience named after you") — the charset makes an email unrepresentable as an audience name, and keying on `subject` instead would let an admin mint themselves read access by naming a key after an audience; a personal audience is now an ordinary audience with an ordinary per-user grant, deferred to Stage 6 (#1374), which is what lets a user mint their own key in the first place. `AudienceReadPolicy::new`/`AudienceMintPolicy::new` now take an `AudienceGrants` instead of an implicit-groups list; `MintPolicy::resolve_audience(caller, None)` is now always an `Err` ("no audience requested and none can be defaulted") — there is no "myself" audience to default to under the opaque-label model. **Migration v6** adds `ingestion_api_keys.audience VARCHAR(255)`, backfilled to `'public'` (the accurate description of every pre-#1372 key's current, unstamped-and-visible-to-everyone state) before `SET NOT NULL`, plus a `CHECK (audience ~ '^[A-Za-z0-9_-]+$')` mirroring `is_valid_audience`. `DbApiKeyAuthProvider`'s loader reads the column back (table-conditional `RETURNING`, `ingestion_api_keys` only) into `bound_audience`; `allow_delegation` is now `false` for ingestion keys (`true`, unchanged, for analytics keys) — a write credential is not a delegating service account, though this is currently inert since an ingestion key can never reach the gRPC path that flag governs. `analytics-web-srv`'s mint/import routes for `ingestion_api_keys` gain an optional `audience` request field and a matching response field (`mint` requires an explicit audience or `MICROMEGAS_DEFAULT_KEY_AUDIENCE` — a **400** if neither resolves, never a silent `public`, since that would publish every future process the new credential ingests; `import` falls back to `public` when neither is set, matching the v6 backfill's continuity assumption for a pre-existing key). New Admin → Ingestion API Keys "Audience" table column and mint-dialog input in the web app (analytics keys are unaffected — they carry no audience). `micromegas-import-keys` gains `--audience` (ingestion only) and a per-key keyring `"audience"` field (wins over `--audience`; combined with `--table analytics` it's a startup error, checked up front rather than partway through a batch of live imports). **Operator-facing break, pre-GA**: `MICROMEGAS_IMPLICIT_GROUPS`/`{prefix}_IMPLICIT_GROUPS` (introduced in v0.29.0, #1369) is removed outright, subsumed by `{"<name>": ["*"]}` in the grant map; a previously-recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=group:everyone` now fails startup under the relaxed charset (`:` is outside `[A-Za-z0-9_-]`) — use `MICROMEGAS_UNSTAMPED_AUDIENCE=public` instead, and drop `MICROMEGAS_IMPLICIT_GROUPS` entirely. **A `NOT NULL` column with no `DEFAULT` also imposes a deploy-order requirement**: roll `analytics-web-srv` to this change in the *same* deploy that runs the v6 migration (ingestion/monolith) — running the migration first without also rolling the web service, or the reverse, both produce an outage window (`NOT NULL` violation, or a missing-column 500, respectively) on the mint/import routes until both sides catch up — see `mkdocs/docs/admin/api-keys.md`. **Minor breaking change**: `micromegas_auth::policy`'s published surface changes shape — `AudienceReadPolicy::new`/`AudienceMintPolicy::new` now take an `AudienceGrants`, and `MintPolicy::resolve_audience`'s `None` contract is now always an error; `micromegas_auth::default_provider::implicit_groups_var` (published) is removed outright. `IngestionKeysState` (published, all-public-fields, `analytics-web-srv`) gains a required `default_audience` field.
* **Ingestion:**
  * Stamp `micromegas.audience` server-side from the authenticated ingestion credential instead of trusting whatever the client sent (#1373, Stage 5 of the epic tracked at #1334) — the fix `ownership_rewrite.rs`'s known Stage 2 limitation pointed at. Every `insert_process`/`register_otel_process` call now resolves a `WriteAudience` from `AuthContext.bound_audience` (`Some` for every DB-backed ingestion key, `None` for env-keyring keys, OIDC, and no-auth-provider deployments) and writes it as the reserved `micromegas.audience` property; any client-supplied property under the reserved `micromegas.*` namespace is dropped at ingestion (`warn!`-logged, naming the key) so the namespace can never be asserted from the payload. `WriteAudience::none()` writes no property at all — absent, not empty, matching what `OwnershipRewriteConfig.unstamped_audience` already coalesces. No retro-stamping: an existing process's audience never changes after the fact. A new `{prefix}_REQUIRE_WRITE_AUDIENCE` knob (falling back to unprefixed `MICROMEGAS_REQUIRE_WRITE_AUDIENCE`, off by default) turns an audience-less credential's write into a 403 (native/OTLP/webhook) or a Firehose retry-then-spill, instead of a silent unstamped write — see `mkdocs/docs/admin/ingestion.md`. One audience per process is now enforced on **both** the native `insert_process` path and the OTLP `register_otel_process` path: a re-registration of an existing `process_id` under a *different* audience than it was originally stamped with is now a 403 (`IngestionServiceError::AudienceConflict`); a re-registration under the same audience, or of a still-`NULL` (never-stamped) row, remains a no-op. This is what makes Stage 2's `MAX(audience)`-per-process resolution (`ownership_rewrite.rs`) sound rather than merely assumed. The `register_otel_process` guard closes a confidentiality gap, not just an integrity one: `processes` is a single table shared with the native path, and since the OTLP `process_id` derivation formula is public, any ingestion credential could pre-register (via the native path) the exact `process_id` a victim audience's OTLP producer would later derive — without this check, the genuine producer's stream/blocks would silently land on a row stamped with the squatter's audience, leaking that audience's data to the squatter. Whichever party registers second gets the 403: in this squatting scenario that's the victim's own later, genuine registration, not the squatter's, and since a stamped process's audience is immutable (no `UPDATE processes` path exists anywhere in the codebase), recovery requires an operator to manually delete the squatted row (e.g. `DELETE FROM processes WHERE process_id = ...`) — the maintenance daemon's `delete_empty_processes` sweep (`rust/analytics/src/delete.rs`) only reclaims it automatically once the squatted row has no streams and the retention window has elapsed.
  * **Known gap, documentation-only for now**: the conflict guard's `NULL`→no-op branch (an existing row with no stamped audience is left alone, so a mid-migration re-registration doesn't lose its process) has its own confidentiality gap, distinct from the squatting gap above: a credential with no bound audience (env-keyring key, OIDC, or `--disable-auth`) can pre-register a victim's future `process_id` unstamped via `insert_process`, and the victim's genuine producer's later registration then hits the same `NULL`→no-op branch and silently no-ops instead of stamping the row — permanently suppressing the victim's stamp. Combined with the commonly recommended `MICROMEGAS_UNSTAMPED_AUDIENCE=public` migration setting, this makes the victim's data world-readable. `{prefix}_REQUIRE_WRITE_AUDIENCE=true` closes it by rejecting the audience-less write that would create the squatted row in the first place.
  * Make OTLP-derived `process_id`/`block_id` audience-scoped (#1373, Stage 5, same epic), closing a structural gap stamping would otherwise expose: `process_id_from_resource` previously hashed only client-supplied resource attributes, so two audiences posting identical resources (the same containerized app in two tenants, a degenerate resource, a CloudWatch namespace) derived the same `process_id` — the first one there would own the row and its audience, silently mislabeling the second's data and, since `blocks` also dedups on content-addressed `block_id` alone, silently dropping byte-identical writes from the second audience entirely. A new `IdentityContext { audience, extra_hash_input }` threads the resolved write audience (and the webhook path's existing header-hash input) into both formulas: `process_id_from_resource` hashes the joined resource-attribute key under a **per-audience namespace UUID** (`NS_OTEL_PROCESS_V1` salted with the audience) when the audience is `Some`, rather than appending the audience string onto the same `\x1F`-joined key the 31 resource fields share — appending it there would have let a resource-attribute value crafted to end in `\x1F<audience>` reproduce another audience's stamped key byte-for-byte, since OTLP attribute values are arbitrary UTF-8 and nothing escapes `\x1F` in them. `block_id`'s hash input gains an `aud\x1F<audience>\x1F` prefix ahead of any `extra_hash_input`. Both formulas are no-ops (byte-identical ids) when the audience is absent, so an unstamped deployment sees zero churn. A deployment that *starts* stamping re-derives its OTLP `process_id`s: the same logical process appears as a new row, and its pre-upgrade data keeps the old id and stays unstamped. Rotating an ingestion key to a different audience likewise splits a long-lived producer's history across two process ids — expected, since the data now genuinely belongs to two audiences.
  * Firehose routes now propagate the `AuthContext` they already validate instead of discarding it: `firehose_auth_middleware` used to authenticate the delivery-stream's access key and then throw the result away (`Ok(_ctx) => { ... }`), so a bound audience on a Firehose credential never reached ingestion. It now inserts the context into the request extensions, exactly like the shared Bearer `auth_middleware` — Firehose and CloudWatch Logs deliveries are stamped and gated the same as every other entry point.
  * **Known gap, tracked as a follow-up (Stage 5b), not closed by this stage**: `insert_stream`/`insert_block` still accept any `process_id`/`stream_id` unconditionally, with no check that the authenticated caller is authorized to write to that specific process. A credential bound to audience A that knows a `process_id`/`stream_id` belonging to audience B can append events to B's process, which then inherit B's stamped audience — an integrity gap, not a confidentiality one (reading B still requires a read grant on B), and distinct from the process-*registration* squatting gap described above, which is a confidentiality issue and is closed by this stage's `register_otel_process` conflict guard. Deferred rather than folded into this stage because the fix shares Stage 3's (#1371) still-unimplemented cache layer; see `insert_stream`/`insert_block_typed`'s doc comments in `web_ingestion_service.rs`.
  * **Minor breaking change**: `WebIngestionService::insert_process` and `register_otel_process` (published, `micromegas_ingestion::web_ingestion_service`) each gain a required `&WriteAudience` parameter; `serve_ingestion` (published, `micromegas::servers::ingestion`) gains a required `StampingConfig` parameter; both `firehose_router`s (`micromegas::servers::firehose` and `::firehose_cloudwatch_logs`) gain a required `Arc<StampingConfig>` parameter; `micromegas_otel_ingestion::identity::process_id_from_resource` and `block::{split_logs, split_metrics, split_traces}` each gain a required `IdentityContext` parameter, and `block::split_logs_with_extra_hash_input` is removed outright, collapsed into `split_logs`; `handler::{ingest_logs, ingest_metrics, ingest_traces, ingest_webhook, ingest_firehose_metrics}` and `cloudwatch_logs::ingest_cloudwatch_logs_firehose` each gain a required `&WriteAudience` parameter; `resolve_write_audience` (published, `micromegas::servers::write_audience`) now takes `Option<&Extension<AuthContext>>` instead of `Option<&AuthContext>`, so every route handler passes its `Option<Extension<AuthContext>>` extractor straight through with no per-call-site unwrap. **Upgrade note**: expect a one-time OTLP `process_id` re-derivation the moment a deployment starts stamping (see above); a process that previously self-stamped `micromegas.audience` on its own while authenticating with an audience-less credential (env-keyring key, OIDC) silently becomes unstamped on upgrade instead of keeping its self-asserted label — move it onto a DB ingestion key bound to that audience to keep its own label. FlightSQL `do_put_statement_ingest` (`CommandStatementIngest`, `bulk_ingest`) now requires an admin credential (`is_admin(request.metadata())`, `Status::permission_denied` otherwise) — `replication.rs`'s `bulk_ingest`/`ingest_processes` write row properties, including `micromegas.audience` on `processes` rows, straight through with no server-side stamping or reserved-namespace stripping (unlike the HTTP ingestion paths above), so any authenticated FlightSQL client could otherwise set `micromegas.audience` directly and bypass this stage's stamping entirely; gating the RPC to admins closes that gap while preserving `bulk_ingest`'s documented purpose (`mkdocs/docs/query-guide/python-api.md`) of replicating a process's audience verbatim from its origin lake. Concretely, "admin credential" means an OIDC-authenticated identity listed in `MICROMEGAS_ADMINS` (or a `--disable-auth` deployment); API-key credentials (both `ingestion_api_keys` and `analytics_api_keys`) are hardcoded to `is_admin: false` and can never satisfy this gate, so any previously-working `bulk_ingest` automation authenticating with an API key now gets a permanent `PERMISSION_DENIED` and must switch to an OIDC-based admin identity.
* **Unreal:**
  * Add external-profiler bridge (`MicromegasExternalProfiler`): when Unreal's external profiler is set to Micromegas, scoped profiler events are forwarded as named spans instead of going through the regular thread-span queue
  * Add `telemetry.global_context_print` and `telemetry.global_context_set_property` console commands to inspect and modify the telemetry global context at runtime
* **Analytics:**
  * Batch JIT partition generation's block queries by observed data density instead of one query per hour, and batch its freshness checks into one query per `generate_*` call instead of one per emitted partition spec (#1474). The old per-hour loop paid a fixed per-query round-trip cost regardless of how little data an hour held, which dominated for sparse view instances queried over long ranges (e.g. OTLP metrics from a single process). Emitted partition specs, and the partitions written from them, are unchanged (byte-identical). **Minor breaking change**: `generate_process_jit_partitions_segment` (published API, `micromegas_analytics::lakehouse::jit_partitions`) is removed, and `JitPartitionConfig` (published API) gains a new field, `target_rows_per_query`; any downstream code calling the former or constructing the latter via a full struct literal needs updating.
  * Fix `thread_spans` queries spanning a JIT segment boundary failing with `declared scan ordering violated` (#1478): a partition's declared event-time bounds come from its blocks' `begin_ticks`/`end_ticks`, and `micromegas_tracing`'s pre-fix flush paths stamp two separate `DualTime::now()`s across a buffer swap, so *every* consecutive block pair in a thread stream strictly overlaps in ticks — not just at the hour-bucket seam this issue reported, but at any `max_nb_objects`-forced cut too. The rows themselves never actually overlap on `begin` (verified at write time by `ensure_begin_non_decreasing`); only the declared bound lied. `lakehouse_partitions` gains a nullable `max_sort_key_time TIMESTAMPTZ` column (schema v7 → v8, no backfill) recording each partition's true maximum `begin`, and `partition_bounds`'s `EventTime` arm now reads it in preference to the looser `max_event_time` when comparing adjacent partitions' bounds in `sort_and_check_non_overlapping` — which the swap-window argument shows can never trip for cuts at block boundaries once a producer shares one flush timestamp (see the `Tracing` entry below). Legacy partitions (written before v8, or by any view that never declares a `Concatenated` event-time ordering) fall back to today's `max_event_time` bound unchanged. `ThreadSpansView::SCHEMA_VERSION` bumps 2 → 3 alongside the migration, so every existing `thread_spans` JIT partition is stale by schema hash and rebuilds automatically — carrying `max_sort_key_time` — on its next query; retiring a previously-failing partition now genuinely fixes it. Not a SQL-visible change beyond one new column: `list_partitions()` gains a trailing, nullable `max_sort_key_time` (plus previously-undocumented `num_rows`/`partition_format_version`/`sort_order` are now documented in `doc/how_to_query/README.md` and `mkdocs/docs/admin/functions-reference.md`); `thread_spans`' queryable schema is unchanged, so existing dashboards and saved queries keep working. **Minor breaking change**: `PartitionRowSet` and `Partition` (published, `micromegas_analytics::lakehouse::write_partition`/`partition`, all-public-field structs) each gain a `max_sort_key_time` field, so any downstream struct literal constructing either needs updating; `PartitionRowSet::new` gains a required third argument (no default, so a future `Concatenated`-declaring view can't silently inherit a wrong `None`); and `write_rows_and_track_times` now returns a `RowSetTrackingResult` struct instead of a bare `Option<TimeRange>`. **Operational note**: as above, `ThreadSpansView::SCHEMA_VERSION` bumping 2 → 3 means every existing `thread_spans` JIT partition rebuilds automatically on its first post-deploy query — no admin action needed, but expect a one-off latency bump on that first query per stream. During a mixed-version rollout, the first new-version node to rebuild a stream retires the old-hash partition out from under any still-running old-version reader that had it cached (self-limiting: that reader's next query rebuilds under its own hash), so prefer a short rollout window, matching #1429's v0.29.0 precedent on this same view.
* **Tracing:**
  * Align the four `micromegas_tracing::dispatch` flush paths (thread/log/metrics/image) with the design intent — and with the Unreal producer — of using one shared timestamp for both the outgoing block's close and the replacement block's `begin`, so consecutive blocks now touch exactly (`block[k].end == block[k+1].begin`) instead of overlapping by the cost of the buffer swap (#1478). `EventBlock` gains a new `close_at(&mut self, end: DualTime)`, used by all four flush paths; the existing self-stamping `close()` is unchanged and still used by standalone-block callers (benches, tests) that swap no stream. `DualTime` additionally derives `Clone`, `Copy`, `PartialEq`, `Eq`.

## v0.29.0 - 2026-08-12

* **Analytics:**
  * Group `thread_spans` and `net_spans` JIT partitions by event time instead of registration order (#1429). Both views build cross-block trees from their source blocks, and `thread_spans` additionally declares `ScanOrdering::Concatenated` over `begin`, but their JIT partitions were cut from the `ORDER BY insert_time, block_id` list — so a stream whose blocks were registered out of event-time order produced partitions holding event-time-interleaved blocks, fragmenting call trees and mis-declaring the scan ordering. A new `JitPartitionConfig::block_order` (`BlockOrder::EventTime`) sorts a segment's blocks by `(begin_ticks, end_ticks)` before cutting, in a single shared `group_blocks_into_partitions` that both orderings now go through; because event-time order can put blocks with out-of-order `insert_time` on either side of a size-based cut, cuts are only taken at *insert-safe* points (every block in the partition being closed inserted no later than every remaining block), falling back to the most recent safe index or, failing that, growing past the soft `max_nb_objects` limit with a `debug!` log. This keeps partitions' insert-time ranges non-overlapping, which the `lakehouse_partitions_no_overlap` exclusion constraint requires. Moving a cut point between runs means a later run's narrower partition can leave a stale, wider one behind that merely *overlaps* it, so these two views retire under a new `RetireMatch::Overlap` predicate — an inclusive-bounds insert-range intersection, `tstzrange(begin, end, '[]') && tstzrange($3, $4, '[]')`, since partition insert ranges are inclusive min/max of block insert times and Postgres's default half-open ranges would miss degenerate and touching shapes — instead of containment alone, with partitions the current `jit_update` run already wrote protected by identity via an explicit `same_run_ranges` list rather than by range shape. `thread_spans` also gained a write-time `ensure_begin_non_decreasing` check on the produced batch. Separately, both views' cross-block call-tree grouping tested `begin_ticks == last_end` for contiguity, which never matched for `micromegas_tracing`-produced streams (that producer stamps the replacement block's `begin` before closing the outgoing block, so consecutive blocks overlap by the cost of the buffer swap) — so call trees never spanned a block boundary for Rust-produced thread streams. The test is now `begin_ticks <= last_end`, since only a *gap* breaks a chain. **Operational note**: both views' `SCHEMA_VERSION` bumps 1 → 2, so every existing `thread_spans`/`net_spans` JIT partition is stale after deploy and rebuilds automatically on first query — no admin action, but expect a one-off latency bump on the first query per stream/process. **Minor breaking change**: `retire_partitions` and `write_partition_from_rows` (published API, `micromegas_analytics::lakehouse::write_partition`) each gain a `retire_match` and `same_run_ranges` parameter; pass `RetireMatch::Containment` and an empty list to keep today's behavior. `is_jit_partition_up_to_date` likewise gains a `BlockOrder` argument.
  * Classify FlightSQL query failures into distinct gRPC status codes instead of always returning `Internal` (#1435): a typo'd function/column, a syntax error, or an unsupported type now comes back as `InvalidArgument`; a query that exceeds a resource budget comes back as `ResourceExhausted`; an unimplemented feature as `Unimplemented`; a genuine server bug stays `Internal`. Classification is derived once via `DataFusionError::find_root()` in a new `classify_datafusion_error`/`client_error`/`classify_flight_error` set of helpers (`rust/public/src/servers/flight_sql_service_impl.rs`), which also drop the old `status!` macro's absolute build-path/file:line suffix entirely (it's simply no longer generated) and add a per-request `query_id` to every client-facing message and to `QueryAuditRecord`, so a failure's client message, its `flightsql_query_audit` log line, and a matching server-side log line (which — unlike the client message — includes the full error and, for execution-time failures with no `Diagnostic` span, the physical plan text, capped and never leaking to the client) can all be correlated by grepping the id. A plan-time error (unknown column, ambiguous reference, type mismatch) now also gets a line/column pointer into the SQL text, via `datafusion.sql_parser.collect_spans` (newly enabled in `make_session_context`). In `datafusion-extensions`, every UDF arity/unsupported-type check that was misclassified as `internal_err!` (`DataFusionError::Internal`, "this should not happen") now uses `exec_err!` (`Execution`) instead, across `jsonb_*`, `color_scale`/`lerp_color`/`rgba`, `lerp`/`unlerp`, `property_get`/`properties_to_array`/`properties_length`, and `bin_center` — needed for the new gRPC mapping to actually help callers, since these are exactly the caller-mistake cases the issue's repro hit. The `flightsql_query_audit` log's `query_failed`/`query_duration_with_error` metrics now fire only for `error_class == "internal"` (a genuine service failure); new `query_failed_user`/`query_failed_resource` counters (count-only, no duration) preserve visibility into the `"user"`/`"resource"` classes without folding them into the service-failure signal — a behavior change for anyone alerting on `query_failed`'s old unconditional rate. The gateway's `/gateway/query` already maps FlightSQL `InvalidArgument` to HTTP 400, so a reclassified bad-query error now surfaces as 400 instead of 500 there too (the web app's own `/query-stream` path is unaffected, since it emits its own hardcoded error frames). pyarrow clients (including `micromegas-query`) now see `ValueError`-subclass exceptions (`ArrowInvalid`/`ArrowNotImplementedError`) for bad queries instead of `FlightInternalError` for everything; see `mkdocs/docs/query-guide/python-api.md`'s Error Handling section. **Minor breaking change**: `QueryAuditRecord` is published API (`micromegas::servers::query_audit`, all-public fields) and gains `query_id` and `error_class`, so any downstream struct literal constructing it needs updating.
  * Add per-query peak memory and disk-spill attribution to the FlightSQL query audit log (#1406): every query now runs against its own `ScopedMemoryPool` — a thin, per-query wrapper `Arc<dyn MemoryPool>` layered over the process-shared pool (`micromegas_analytics::lakehouse::scoped_memory_pool::ScopedMemoryPool`) — so a reservation is welded to the pool instance it registered with regardless of which task or thread later grows it, with no ambient context or per-consumer naming needed. `QueryAuditRecord` gains `peak_memory_bytes` (the wrapper's high-water mark, so it's valid even on `error`/`incomplete` records), `spilled_bytes`, and `spill_count` (summed from the physical plan tree via `MetricsSet::spill_count()`/`spilled_bytes()`, not `sum_by_name`, which silently returns zero for those two). Also emits a `query_peak_memory_bytes` metric from every terminal path. **Minor breaking change**: `QueryAuditRecord` and `ScanMetrics` are published API (`micromegas::servers::query_audit`, all-public fields) and gain three and two new fields respectively, so any downstream struct literal constructing them needs updating.
  * Add `MICROMEGAS_DATAFUSION_MAX_TEMP_DIRECTORY_MB` to cap DataFusion's total spill-file usage across all concurrent queries (default: DataFusion's own 100 GB, unchanged if unset) — see `mkdocs/docs/admin/flight-sql.md`/`maintenance.md`/`monolith.md`.
  * Fix `HistogramAccumulator::size()` under-reporting memory usage to DataFusion's memory pool by up to ~7x (worse for larger `nb_bins`): it now reports the `bins` `Vec`'s allocated capacity via `VecAllocExt::allocated_size()` instead of the `Vec` header's stack size, so `MICROMEGAS_DATAFUSION_MEMORY_BUDGET_MB` spills grouped histogram aggregates correctly instead of risking an OOM kill (#1448)
  * Add client attribution headers to the FlightSQL query audit log (#1436): `x-client-agent` (who is driving the client, e.g. `claude-code`, auto-detected from known agent-harness marker env vars), `x-client-entrypoint` (how the client was invoked: `script`, `jupyter`, `repl`, or the CLI's explicit `cli-query` label), and `x-client-session` (an opaque id correlating every query issued through one client instance/session), each read alongside the existing `x-client-type` header in `flight_sql_service_impl.rs::execute_query`, defaulting to `"unknown"` (`agent`/`entrypoint`) or omitted (`session`) when absent. `agent`/`entrypoint` are also added to the start-of-query `info!` log line. The three headers are forwarded through the HTTP gateway's default header allowlist unchanged (`build_origin_metadata` still only augments `x-client-type`/generates `x-request-id`, since agent/entrypoint/session describe who authored the SQL, not which hops the request took) — see `mkdocs/docs/query-guide/query-audit-log.md` and `mkdocs/docs/gateway/configuration.md`. **A deployment with a custom `MICROMEGAS_GATEWAY_HEADERS` allowlist must add `X-Client-Agent`, `X-Client-Entrypoint`, and `X-Client-Session` to that JSON explicitly, or it will keep dropping them after upgrading** (`HeaderForwardingConfig::from_env()` replaces the allowlist entirely rather than merging with it). **Minor breaking change**: `QueryAuditRecord` is published API (`micromegas::servers::query_audit`, all-public fields) and gains `agent`, `entrypoint`, and `session`, so any downstream struct literal constructing it needs updating.
  * Advertise the originating notebook and cell on web-app FlightSQL queries (#1437): `analytics-web-srv`'s `/api/query-stream` endpoint now accepts optional `notebook`/`cell` fields on `StreamQueryRequest`, sanitizes them (bounded-length, printable-ASCII, reject-whole-value on failure — same strategy as the Python client's `_sanitize_override` from #1436) via a new `sanitize_origin_label`, and forwards them as `x-client-notebook`/`x-client-cell` gRPC metadata headers through a new generic `BearerFlightSQLClientFactory::with_metadata()` builder. `flight_sql_service_impl.rs::execute_query` reads both alongside the existing client-attribution headers, adds them to the start-of-query `info!` line, and threads them into `QueryAuditRecord` as `notebook`/`cell` (both `Option<String>`, omitted when the query didn't originate from a notebook cell — e.g. the standalone query editor). Forwarded through the HTTP gateway's default header allowlist alongside the #1436 headers. **A deployment with a custom `MICROMEGAS_GATEWAY_HEADERS` allowlist must add `X-Client-Notebook` and `X-Client-Cell` to that JSON explicitly, or it will keep dropping them after upgrading.** **Minor breaking change**: `QueryAuditRecord` is published API (`micromegas::servers::query_audit`, all-public fields) and gains `notebook`/`cell`, so any downstream struct literal constructing it needs updating.
  * Add `client_ip` to the FlightSQL query audit log (#1459): `QueryAuditRecord` gains a `client_ip` field, resolved once per RPC in `do_get_fallback`/`do_get_statement` and threaded through `execute_query`'s new `client_ip: &str` parameter into both start-of-query `info!` lines and the audit record. The value itself comes from a change to the shared `get_client_ip` (`rust/public/src/servers/http_utils.rs`), which now selects the *rightmost* entry of the *last* `X-Forwarded-For` header field line instead of the leftmost: every service sits behind an AWS ALB, which *appends* the address it observed rather than overwriting the header, so the old leftmost read let any caller spoof `client_ip` outright by prepending a value of their choice — the rightmost entry is the ALB's own observation and can't be forged for traffic that actually traverses it. **This is a behavior change for every existing `client_ip` logger, not just an addition**: `observability_middleware` (`/api/*` request/response lines, and the ingestion service's `/ingestion/*`, OTLP, webhook, Firehose, and `/auth/api_keys` request/response lines), `LogUriService` (FlightSQL's generic per-RPC line), and the HTTP gateway's forwarded `x-client-ip` metadata header all switch from the caller-supplied leftmost entry to the ALB-observed rightmost one — a deployment not sitting behind exactly one appending proxy will see different values than before. `client_ip` reports the *proxying* service's own address, not the original caller's, for FlightSQL calls made through the HTTP gateway's `/gateway/query` or `analytics-web-srv`'s `/api/query-stream`, since neither hop forwards the caller's `X-Forwarded-For` chain today (documented in `mkdocs/docs/query-guide/query-audit-log.md` and `mkdocs/docs/gateway/index.md`); direct FlightSQL access is unaffected. **Minor breaking change**: `QueryAuditRecord` is published API (`micromegas::servers::query_audit`, all-public fields) and gains `client_ip`, so any downstream struct literal constructing it needs updating.
  * `parse_block(block_id)` now decodes OTLP blocks (`otlp/v1/logs`, `otlp/v1/metrics`, `otlp/v1/traces`), not just `micromegas-transit` ones (#1467): the hard-coded format check is replaced with a format → decoder registry (`BlockObjectDecoderMap`, mirroring the existing `BlockProcessorMap` pattern, in new module `micromegas_analytics::lakehouse::block_object_decoder`), and new `otel/block_decoders.rs` decoders walk each OTLP payload's `scope_* → leaf records` to emit one row per leaf (log record / span / metric data point — Sum/Gauge/Histogram/ExponentialHistogram/Summary each contributing their own data points), same `(object_index, type_name, value JSONB)` shape as transit blocks. `value` is a faithful OTLP/JSON dump of the leaf (`serde_json` + `jsonb::Value::from`, so field naming/64-bit-nanos/trace-and-span-id-hex match the wire form directly) with a synthesized `__`-prefixed envelope layered on top: `__type`, `__attributes` (leaf attrs, flattened), `__resource` (resource attrs, flattened), `__scope` (`scope_extras`'s `otel.scope.*` keys), and — metrics only — `__metric` (the parent `Metric`'s `name`/`unit`/`description`, `otel.metric.kind`, and `aggregation_temporality`/`is_monotonic`/`metadata` where applicable, none of which the data point carries itself). `attrs_to_jsonb_value` is extracted from `otel/attrs.rs`'s existing `attrs_to_jsonb` (now a one-line wrapper) to build `__attributes`/`__resource` without duplicating the flattening logic. This closes a real diagnostic gap: previously an OTLP block that materialized to an empty `log_entries`/`measures`/`otel_spans` row gave no SQL-reachable way to tell "the JIT/daemon pipeline is stalled" from "the payload doesn't contain what we think it does" — `parse_block` now answers that directly, e.g. `SELECT jsonb_as_string(jsonb_get(jsonb_get(value, '__attributes'), 'my.event.id')) FROM parse_block('<block_id>')`. Two related fixes to `parse_block` land alongside the decoder work: (1) a block absent from `blocks` for the queried time range now returns a clear error instead of silently returning zero rows — the common case given `micromegas-query` requires `--begin`/`--all` and `blocks`/partition pruning both apply the query range; and (2) `block_id` is validated and canonicalized (`Uuid::parse_str` + `to_string()`) before the lookup, so braced/URN/bare-hex forms of a valid id no longer silently miss. Both new error paths, plus the "no decoder for streams.format" case (which now lists known formats, built from the decoder map itself, instead of a single hardcoded name), surface as DataFusion `Plan` errors — `InvalidArgument`/`error_class="user"` at the FlightSQL layer, not `Internal`. See `mkdocs/docs/query-guide/functions-reference.md` and `mkdocs/docs/otlp/index.md` for the updated docs, including known lossy-conversion notes (non-finite `f64` → JSON `null`; `asInt`/`Exemplar.as_int` as bare numbers, not quoted strings) and the OTLP-specific caveat that `object_index` is positional only and not guaranteed to match `nb_objects` (which over-counts Summary data points by design).
* **Auth:**
  * Add a DB-backed API key store (#1383): `ingestion_api_keys` / `analytics_api_keys` (data-lake schema v5), each holding only a SHA-256 hash of the key plus a `created_at`/`created_by`/`last_used_at`/`revoked_at`/`revoked_by` audit trail, validated through `DbApiKeyAuthProvider` behind a short-TTL `moka` cache. Mint/list/revoke/import for both this table and `analytics_api_keys` live entirely on `analytics-web-srv`'s admin routes (see #1411/#1458 below) — ingestion itself exposes no key-management HTTP surface at all, it only validates incoming keys against `ingestion_api_keys` — see `mkdocs/docs/admin/api-keys.md`. Four new cache/audit env knobs, each with a `{prefix}_*`-with-unprefixed-fallback form: `MICROMEGAS_API_KEY_CACHE_SIZE`, `MICROMEGAS_API_KEY_CACHE_TTL_SECONDS`, `MICROMEGAS_API_KEY_UNKNOWN_CACHE_TTL_SECONDS`, `MICROMEGAS_API_KEY_UNKNOWN_CACHE_SIZE`. A new `db_api_key_error_count` metric (tagged `{table}`) fires on every key-store DB error, independent of the rate-limited `error!` log for the same error.
  * A DB-backed key-store outage now surfaces as a retryable failure, not a rejected credential, at every surface a DB-backed provider reaches: a new `micromegas_auth::types::ProviderUnavailable` propagates through `MultiAuthProvider` to a new `micromegas_auth::axum::AuthError::Unavailable` (**503**, instead of the previous unconditional 401), to `Status::unavailable` in the gRPC `AuthService` (instead of `Status::unauthenticated`), and to a 503 in the Firehose auth middleware (instead of the previous unconditional 401). **Minor breaking change**: `AuthError` is published API (`rust/auth`, no `publish = false`) with a new variant, so any downstream exhaustive `match` needs a new arm.
  * **Public-API removal**: `micromegas::servers::key_ring` (a dead, unreferenced duplicate of `api_key.rs`'s keyring half) is deleted. `micromegas::servers::api_keys` (its `api_keys_router` function and `ApiKeyError` type) is also deleted, superseded by the mint/list/revoke/import routes below (#1411/#1458).
  * Add mint/list/revoke/import HTTP routes for both `analytics_api_keys` and `ingestion_api_keys`, hosted entirely on `analytics-web-srv` (#1411, revised by #1458): `POST`/`GET`/`DELETE /api/analytics-api-keys[/{key_id}]` and `POST /api/analytics-api-keys/import`, plus the equivalent `POST`/`GET`/`DELETE /api/ingestion-api-keys[/{key_id}]` and `POST /api/ingestion-api-keys/import`, all gated by the same cookie/bearer-auth admin check every other `analytics-web-srv` admin route uses. `ingestion_api_keys` mint/list/revoke/import are direct Postgres writes over the same narrowly-scoped `max_connections(2)` telemetry-DB pool `analytics_api_keys` already used — there is no proxy to ingestion and no second OIDC service credential; `MICROMEGAS_INGESTION_ADMIN_URL` and `MICROMEGAS_INGESTION_PROXY_OIDC_CLIENT_ID`/`_CLIENT_SECRET`/`_TOKEN_ENDPOINT`/`_AUDIENCE`, an earlier iteration's proxy-credential env vars, are gone — a deployment that still sets them just has them silently ignored. **Ingestion itself exposes no key-management HTTP routes at all** — no `/auth/api_keys*`, no `serve_ingestion_with_api_key_config`; it only validates keys against `ingestion_api_keys`, and `serve_ingestion`'s signature is unchanged. Writing directly instead of proxying also fixes an attribution bug: every ingestion-key mint/revoke/import now records the acting admin's own OIDC identity (`created_by`/`revoked_by`), never a shared service credential. Two new admin pages in the web app (Admin → Ingestion API Keys / Analytics API Keys), each showing a minted key exactly once in a dismissable banner with copy-to-clipboard. **Under `--disable-auth` on `analytics-web-srv`, both key-management route groups are structurally unreachable, not just gated** — a static 503 router answers both path prefixes (and any sub-path) instead, since a hardcoded admin `ValidatedUser` in that mode would otherwise let any unauthenticated caller mint/revoke real keys. New `micromegas-import-keys` CLI console script (alongside `micromegas-query`/`-screens`/`-logout`) walks a legacy env keyring (`--source env --var NAME` or `--source file --path ...`, `--only`/`--exclude` to select entries) and calls the import routes above directly (no `psql`) — see `mkdocs/docs/admin/api-keys.md`. **Minor breaking change**: `WebServerConfig` (published, all-public-fields API in `analytics-web-srv`) gains `analytics_keys_db_string`, so any downstream struct literal constructing it needs updating.
  * **One client-visible breaking change**: a key valid on both ingestion and flight-sql today must become two distinct keys once migrated onto the DB-backed store — see `mkdocs/docs/admin/api-keys.md`.
  * **Operator-visible consequence of #1458**: since `analytics-web-srv` now writes `ingestion_api_keys` directly instead of proxying to ingestion, its DB role (`micromegas_web`) needs `SELECT`/`INSERT` and `UPDATE (revoked_at, revoked_by)` grants on `ingestion_api_keys` too, alongside its existing grants on `analytics_api_keys` — see `mkdocs/docs/admin/api-keys.md`'s grants section. `micromegas-import-keys --table ingestion --url` must now point at `analytics-web-srv`'s base URL (previously ingestion's own), matching `--table analytics`; the standalone `micromegas.ingestion_client.IngestionClient` (its only caller) is removed. `micromegas_ingestion`'s own DB grant narrows to `SELECT` plus `UPDATE (last_used_at)`, since minting/revoking no longer runs on ingestion. **This also silently and irreversibly expands who can administer ingestion keys**: the old proxy only forwarded ingestion-key mint/revoke/import when an operator configured `MICROMEGAS_INGESTION_ADMIN_URL` plus the `MICROMEGAS_INGESTION_PROXY_OIDC_*` quartet, and leaving those unset was the documented way to keep ingestion's admin list separate from `analytics-web-srv`'s own (`MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS`). That opt-out is gone — ingestion-key administration now hangs unconditionally off the same `MICROMEGAS_SQL_CONNECTION_STRING`-backed pool as `analytics_api_keys`, so any deployment already using that connection string for analytics-key admin gains ingestion-key admin for the same admin list on upgrade, with no remaining knob to disable it short of unsetting `MICROMEGAS_SQL_CONNECTION_STRING` entirely. Operators who relied on the old opt-out to keep the two admin lists separate must re-audit `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS` before upgrading — see `mkdocs/docs/admin/api-keys.md`.
  * Fix `/auth/*` routes (`login`/`callback`/`refresh`/`logout`/`me`) never getting `client_ip`-tagged request/response logging (#1459): `analytics-web-srv`'s `build_auth_routes` is now wrapped in a new `auth_observability_middleware` (`micromegas::servers::axum_utils`) — a variant of `observability_middleware` (both now share one `observability_middleware_impl`) that logs only `uri.path()`, never the query string, since `/auth/callback`'s query carries the live OAuth authorization code and the signed `state` (which embeds the PKCE verifier) and must never land in `log_entries`. `build_auth_routes` is now `pub`, matching the `build_protected_routes` precedent, so `routing_tests.rs` can exercise the real, layered router directly. Also fix `[auth_success]` audit lines on login/token-refresh logging only an opaque `sub`: a new `extract_audit_claims_from_token`/`AuditClaims` (replacing `extract_subject_from_token`, both now `pub`, re-exported from `analytics_web_srv::auth`) reads `sub` and `email` from the unverified JWT payload in one pass, so `auth_callback`'s/`auth_refresh`'s `[auth_success] event=login|token_refresh sub=... email=...` lines now include the email claim already present in the same token (the OIDC client always requests the `email` scope), matching `cookie_auth_middleware`'s existing `email={:?}` logging convention.
  * Add the AbAC authorization seam (#1369, Stage 1 of the epic tracked at #1334) — **no enforcement, no behavior change**; the new knob is unset by default and nothing consumes it yet. New `MintPolicy`/`ReadPolicy` traits and their audience-based implementations, `AudienceMintPolicy`/`AudienceReadPolicy` (`micromegas_auth::policy`), resolve a caller's readable-audience set from `AuthContext` (email/`groups` claim/implicit groups/a service-account `read_audiences` grant) — `ReadPolicy::resolve` can never return "all", and any `Err` from it is a hard failure (`Status::unavailable`/`Status::permission_denied`), never a defaulted scope. `AuthContext` gains `groups: Vec<String>` (populated from a flat OIDC `groups` claim), `bound_audience`/`read_audiences` (both empty until Stages 4/4b populate them). On the query-planner side, a new `ReadScope`/`CallerContext` (`micromegas_analytics::lakehouse::read_scope`) replaces the `is_admin: bool` parameter threaded into `make_session_context`, carrying the resolved scope alongside the existing admin-gate flag; nothing consumes `read_scope` yet (Stage 2/3 are its first consumers). `FlightSqlServiceImpl` resolves the caller's `AuthContext` from the gRPC request extension `AuthService` already inserts (never from the client-facing `x-user-*` attribution headers) and closes three identity holes: prepared statements (`do_action_create_prepared_statement`) now resolve the same identity as `do_get`, instead of no identity at all; `analytics-web-srv`'s cookie middleware now inserts the full `AuthContext` into request extensions alongside `ValidatedUser`, since it has been the mint path's identity source since #1458 and a future `MintPolicy` consulting `groups` needs it; and `UserAttribution`'s doc comment now states explicitly that it must never feed a `ReadPolicy`. New env knob `{prefix}_IMPLICIT_GROUPS` (falling back to unprefixed `MICROMEGAS_IMPLICIT_GROUPS`), comma-separated, unset by default. **Minor breaking change**: `FlightSqlServiceImpl::new` (published API, `micromegas::servers::flight_sql_service_impl`) gains a required `read_policy: Arc<dyn ReadPolicy>` parameter; `make_session_context`, `register_functions`, `register_lakehouse_functions`, and `query()` (all published, `micromegas_analytics::lakehouse::query`) replace their `is_admin: bool` parameter with a `CallerContext` (`CallerContext::internal()`/`::maintenance()` for the two common cases). `FlightSqlServerBuilder` gains `with_read_policy()`. `AuthContext` (published, `micromegas_auth::types`, all-public fields, not `#[non_exhaustive]`) gains `groups`, `bound_audience`, and `read_audiences`, so any downstream struct literal constructing it needs updating.
* **Web App:**
  * Fix notebook cells failing with `Unrecognized type: "undefined" (24)` whenever a query returns an Arrow `Utf8View` column — which modern DataFusion produces from ordinary string functions like `LEFT(...)`/`replace(...)` — or a `BinaryView` column: bump `apache-arrow` from `21.1.0` to `21.2.0`, which adds decode support for both view types across both decode paths (the streaming `RecordBatchReader` used for server queries, and the whole-buffer `tableFromIPC` used for the in-browser `datafusion-wasm` and `fetchQueryIPC` cell paths). Also extend `arrow-utils.ts`'s `isStringType`/`isBinaryType` predicates to recognize `Utf8View`/`BinaryView` (via arrow-js's own `DataType.isUtf8View`/`isBinaryView` statics), so a decoded view-type column is no longer silently rejected as an unsupported type by chart axes, swimlane color-by, and map color-by, and a `BinaryView` column renders as the ASCII preview in tables instead of comma-joined byte numbers (#1294)
  * Fix chart Y-axis auto-scaling breaking when a series contains a non-finite value (`Infinity`/`-Infinity`), e.g. from a SQL ratio dividing by zero: `arrow-utils.ts`'s chart-data extraction now drops non-finite X/Y values the same way it already drops nulls, instead of letting one `Infinity` blow up the computed axis range and flatten every other point. Extends the same guard to the perf-analysis and process-metrics pages' own row extraction (both reachable via arbitrary user-typed SQL), which previously had no null/NaN/finiteness guard at all (#1424)
  * Fix data source URL validation rejecting `grpc://`/`grpc+tls://` even though data sources are only ever used as FlightSQL/gRPC endpoints: `validate_data_source_config` now accepts `grpc://`, `grpc+tls://`, `http://`, and `https://` (case-insensitive), matching the scheme convention used elsewhere in the codebase (e.g. the Python client's default `grpc://localhost:50051`); `BearerFlightSQLClientFactory::make_client()` normalizes `grpc://`/`grpc+tls://` to `http://`/`https://` before building the tonic channel so a `grpc+tls://` data source actually gets a TLS-enabled connection instead of silently connecting without TLS (#1412)
  * Fix object-valued metric properties (e.g. CloudWatch `Dimensions`) rendering as the literal string `[object Object]` in the per-process metric property timeline: a new `flattenProperties` helper in `property-utils.ts` expands an object-valued property into dot-separated leaf keys (e.g. `Dimensions.DBInstanceIdentifier`) at parse time, applied at all three JSON-parsing call sites — `property-utils.ts`'s `extractPropertiesFromRows`, `useMetricsData.ts`, and `ProcessMetricsPage.tsx`. The same helper also JSON-stringifies top-level array-valued properties (e.g. `Tags: ["a","b"]` now renders as `["a","b"]`) instead of relying on `String(value)`'s comma-join behavior, which previously lost structure for arrays of objects (rendering `[object Object]`). Accepted migration gap: a `selectedProperties`/`selectedKeys` value saved before this fix that names a pre-flatten object-valued key (e.g. plain `Dimensions`) no longer matches anything post-fix — only its `Dimensions.<leaf>` descendants exist — so that saved row now renders with no segments and no explanation, instead of the old `[object Object]` (#1390)
  * Add a tab favicon indicator for screen execution state: `ScreenPage`'s existing page-global `isExecuting` (already fed by every renderer — table/log/metrics streaming queries, process-list, and notebook cell execution) now also drives a swapped `<link rel="icon">` — a busy badge while running, an error badge if the screen finished with an error, reverting to the normal icon once idle — so a user who tabs away from a long-running notebook cell or query can tell its outcome without switching back. New `useTabExecutionState` hook resets the favicon on unmount (leaving the screen-route family) and `ScreenPage`'s `load()` effect resets the state on screen-to-screen navigation, so neither leaves a stale busy/error icon behind (#1443)
  * Fix the sidebar's search box collapsing the flyout back to the icon rail on the first character typed from any page other than `/screens`: `Sidebar` was mounted fresh inside every route's own `PageLayout`, so navigating to `/screens` (triggered by that first keystroke) unmounted and remounted it, resetting its open/closed flyout state. `Sidebar` now mounts once in a new `AppShell` layout route above `<Routes>`, gated on auth status so it doesn't mount during loading/unauthenticated/admin-denied states, persisting across navigation instead (#1439)
  * Fix categorical chart X-axis labels overlapping into an unreadable smear when they're long strings (e.g. version/build identifiers): `buildXAxisConfig`'s categorical branch now tilts labels to -45° and grows the axis to fit them, but only once per-tick space is too narrow to fit them horizontally — short labels keep rendering flat. Also reserves right-edge `padding` for the last rotated label's overflow (capped, and reduced when a right y-axis already provides clearance), and lowers the per-tick `space` floor once rotated so the padding fix doesn't itself blank the axis on charts that render correctly today (#1425)
  * Attribute notebook-cell queries in the FlightSQL query audit log with the originating notebook and cell name (#1437): `StreamQueryParams` (the shared type both `streamQuery()` and `fetchQueryIPC()` funnel through) gains optional `notebook`/`cell` fields, threaded from `ScreenPage`'s `screen?.name` through `NotebookRenderer`'s new `screenName` prop into `useCellExecution` (all four query call sites in `executeCell`) and into `PerfettoExportCell`'s separate `fetchPerfettoTrace()` path (which bypasses `useCellExecution` entirely). Cells are identified by name, same as the existing `migrateCellState`/`removeCellState` convention; a query issued outside a notebook (the standalone query editor, other screen types) omits both fields and stays plain `client=web`, unchanged from today.
  * Add a Pie Chart notebook cell type for visualizing proportions/breakdowns from a single SQL query returning `(category, value[, color])`: renders as a pie or donut (donut default, with a center total) via a hand-rolled inline SVG, with an always-visible legend, per-slice tooltips, and a `Max Slices` cap that folds the smallest categories into an "Other" slice (#1339)
  * Fix the Pie Chart cell's legend stretching to fill the full card width instead of sizing to its content
* **Python:**
  * **Breaking change**: remove the deprecated `MICROMEGAS_PYTHON_MODULE_WRAPPER` escape hatch, which let a corporate environment plug a custom `connect()` implementation into the CLI, bypassing the documented URI/OIDC connection path entirely. Corporate auth is now served by the OIDC flow (`MICROMEGAS_OIDC_ISSUER`/`MICROMEGAS_OIDC_CLIENT_ID`/etc., or `~/.micromegas/config.json`'s `issuers` config) — anyone still setting the env var should switch to that instead (#1408)
  * Add AWS-CLI-style named connection profiles to `micromegas-query`/`micromegas-logout`: an optional `profiles` map in `~/.micromegas/config.json` holds more than one named connection (prod/dev/local), selected by `--profile` or `MICROMEGAS_PROFILE` (precedence: `--profile` > `MICROMEGAS_PROFILE` > `default_profile`), with each profile caching its OIDC tokens separately at `~/.micromegas/tokens-<profile>.json` so switching profiles doesn't reuse another environment's cached token. Existing flat config files (no `profiles` key) keep working untouched. A `profiles` map always requires a selected profile (no implicit single-profile selection); an unresolvable selection now raises a clean CLI usage error instead of a traceback. **Behavior change**: a bare `micromegas-logout` now clears every cached token file (the plain `tokens.json` plus every `tokens-<profile>.json`), not just `tokens.json`; pass `--profile <name>` to clear only one profile's token. **Breaking change**: the `MICROMEGAS_TOKEN_FILE` env var is removed — the token cache path is now always derived from the active profile (`config.py:default_token_file`); anyone exporting it logs in once more, with tokens landing in the default location (#1403)
  * Add `--version` to `micromegas-query`, `micromegas-logout`, and `micromegas-screens`, and expose `micromegas.__version__`, so it's easy to tell which installed wheel (and which interpreter) backs a console script; `--version` reports the package version plus the interpreter version and path (e.g. `micromegas-query 0.29.0 (Python 3.11.9 at /usr/bin/python3.11)`), reading the package version from `importlib.metadata.version("micromegas")` via a shared `micromegas.cli.version` helper, falling back to `"unknown"` if the package isn't installed. Also fix `micromegas-query`'s and `micromegas-logout`'s `prog=` value (previously `query` / `micromegas_logout`) to match their actual console-script names (#1416)
  * Send three new client attribution headers on every FlightSQL query, resolved once per `FlightSQLClient` instance (#1436): `x-client-agent` (who is driving the client, auto-detected from known agent-harness marker env vars — currently just Claude Code's `CLAUDECODE`, reported as `claude-code`; `"none"` otherwise, overridable via `MICROMEGAS_CLIENT_AGENT`), `x-client-entrypoint` (how the client was invoked — auto-detected `script`/`jupyter`/`repl`, overridable via the new `client_entrypoint` constructor parameter or `MICROMEGAS_CLIENT_ENTRYPOINT`), and `x-client-session` (an opaque id correlating every query from one client instance — a fresh UUID per instance, unless a known agent harness's session env var, currently just Claude Code's `CLAUDE_CODE_SESSION_ID`, is present, in which case that value is reused verbatim so a fresh-per-invocation CLI process still correlates across queries within the same agent session). New `micromegas.flightsql.attribution` module does the detection; `micromegas-query` passes the explicit `client_entrypoint="cli-query"` label. All three are analytics-only signals, never used for auth/quota/rate-limiting, and trivially spoofable/omittable like the existing `x-client-type` — see `mkdocs/docs/query-guide/python-api.md`'s new "Client Attribution" subsection. `FlightSQLClient.__init__`/`oidc_connection.connect`/`cli/connection.connect` each gain an optional `client_entrypoint` parameter (non-breaking, defaults preserve prior behavior).
* **Security:**
  * Bump `undici` to `8.10.0` via yarn `resolutions`/`overrides` across all yarn workspaces (root, `analytics-web-app`, `welcome`, `doc/intro-micromegas`, `doc/notebooks`, `doc/unified-observability-for-games`) to resolve Dependabot alerts 403-433 (GHSA-m8rv-5g2x-5cg5, GHSA-jr45-8vmc-qm54, GHSA-v3r7-h72x-cjcm, GHSA-8xcm-r25x-g524, GHSA-4cwx-7wf7-3272), and bump `fast-uri` to `3.1.5` to resolve Dependabot alert 428 (GHSA-7p8r-x3mc-p8w7)
  * Bump `cryptography` to `50.0.0` in `python/micromegas` to resolve Dependabot alert 434 (GHSA-g6cj-pr64-35w5, CVE-2026-69247), a Bleichenbacher oracle in PKCS#7 `EnvelopedData` decryption
  * Bump `event-listener` (transitive, via `async-lock`/`moka`/`sqlx-core`) from `5.4.1` to `5.4.2` to resolve RUSTSEC-2026-0221 (unsound `!Send` tags crossing thread boundaries via `StackSlot`), and `spin` (transitive, via `lazy_static`) from the yanked `0.9.8` to `0.9.9`
  * Bump `js-yaml` to `4.3.1` at the repo root to resolve Dependabot alerts 445-446 (GHSA-5p4m-2wfm-xmqj, quadratic CPU consumption in `!!omap` resolution), and bump `mermaid` to `11.16.1` in `doc/high-frequency-observability` and `doc/intro-micromegas` to resolve Dependabot alerts 435-444 (DoS in radar/XY-chart diagrams, prototype pollution in configuration APIs and Architecture diagrams, CSS injection into sibling elements)
  * Bump `nanoid` (transitive, via `postcss`) to `^3.3.17` via yarn `resolutions`/npm `overrides` across the root, `doc/unified-observability-for-games`, `doc/notebooks`, `doc/intro-micromegas`, and `doc/high-frequency-observability` workspaces to resolve Dependabot alerts 451, 452, 453, 454, 456 (GHSA-2v37-7h3g-55p8, custom generators can loop indefinitely when size is zero)
* **Docs:**
  * Document the `aws.event.id`/`aws.event.time` attribute convention for the EventBridge `input_transformer` → OTLP/JSON API Destination ingestion path, analogous to the existing `aws.log.event.id` convention for CloudWatch Logs/Firehose, and cross-link it from the `Idempotency` section to the content-hash `block_id` collision incident (#1462) and the open producer-declared idempotency-key proposal (#1466) (#1470)
  * Fix the Grafana plugin's Go prerequisite in `CONTRIBUTING.md`/`mkdocs/docs/contributing.md` (stale "Go 1.23+", but `grafana/go.mod` and the `grafana-plugin` CI workflow both require 1.25) and document why `mage coverage` can fail with `go: no such tool "covdata"` on a fresh setup, plus the one-time fix (building `cmd/covdata` into `GOROOT`'s tool directory)
  * Document in the Build Guide's Prerequisites that `local_test_env`'s PostgreSQL-management scripts (`start_services.py`, `local_test_env/db/run.py`) require the Python `docker` SDK package (`pip install --user docker`), and warn against Ubuntu noble's `python3-docker` apt package, which is pinned at docker-py 5.0.3 and breaks with a `URLSchemeUnknown: http+docker` error once urllib3 2.x is present
* **Build:**
  * Bump `date-fns` from 2.30.0 to 4.4.0 in `analytics-web-app`. This is a deduplication as much as an upgrade: `react-day-picker@9.14.0` already depended on `date-fns@^4.1.0`, so the lockfile carried both majors side by side. All five functions the app imports (`format`, `setHours`, `setMinutes`, `startOfDay`, `endOfDay`, at `src/components/ui/DateTimePicker.tsx`) are unchanged in v4 (#1255)
  * Migrate `analytics-web-app` from `eslint` 8.57.1 (end-of-life) to 10.8.0, which required moving `.eslintrc.json` to flat config (`eslint.config.js`) since ESLint 10 removes eslintrc support entirely. Replaces the split `@typescript-eslint/{eslint-plugin,parser}` ^7 with the unified `typescript-eslint` ^8, bumps `eslint-plugin-react-hooks` to ^7 and `eslint-plugin-react-refresh` to ^0.5, and drops the dead `@eslint/eslintrc` devDep, the now-orphaned `js-yaml` `resolutions` pin, and the `@typescript-eslint/utils@7.18.0` `packageExtensions` entry in `analytics-web-app/.yarnrc.yml`. Crossing 8 → 10 adds seven rules to `eslint:recommended` and changes `no-constant-condition`'s `checkLoops` default; flat config also enables `reportUnusedDisableDirectives` by default, which surfaced 12 stale `eslint-disable` comments (now removed). `eslint-plugin-react-hooks` 7 bundles the React Compiler rule family, which reports 106 findings across 48 files in this codebase — those five rules (`refs`, `set-state-in-effect`, `static-components`, `immutability`, `purity`) are disabled for now, so this bump stays a dependency bump (#1255)
  * Enable the five React Compiler `eslint-plugin-react-hooks` rules deferred in #1255 (`refs`, `set-state-in-effect`, `static-components`, `immutability`, `purity`) at `error`, with zero findings aside from five justified per-site `react-hooks/refs` disables (#1423)
  * Migrate `analytics-web-app` from Tailwind CSS 3.4.19 to 4.3.3, and `tailwind-merge` 2.6.1 to 3.6.0 (v2 encodes v3's class taxonomy, so it must move in lockstep). Switches to `@tailwindcss/vite`, removing `postcss.config.mjs` along with the `autoprefixer` devDep it required, plus the direct `browserslist`/`baseline-browser-mapping` devDeps and the `baseline-browser-mapping` `resolutions` pin, no longer needed as direct declarations now that `autoprefixer` is gone (both packages remain in the tree transitively via `@vitejs/plugin-react` → `@babel/helper-compilation-targets` → `browserslist` → `baseline-browser-mapping`); the existing `tailwind.config.ts` is kept via the `@config` directive rather than ported to a CSS `@theme` block. **This raises the browser floor to Safari 16.4+ / Chrome 111+ / Firefox 128+**, since v4 depends on `@property` and `color-mix()`. Other user-visible consequences: `hover:` styles are now wrapped in `@media (hover: hover)` and so no longer apply on touch-primary devices (295 sites); opacity modifiers on the theme's bare `var(--…)` colors, which v3 silently dropped, now render as real `color-mix()` (~106 sites that previously showed no background, ring, or border); and `space-*`/`divide-*` switch to a `:where(… > :not(:last-child))` selector with `margin-block-end`, so `[hidden]` children now contribute spacing and child-level margin utilities win on specificity (66 sites). `borderRadius.xs` and `fontFamily.sans` were added to the config to keep v4's changed defaults value-preserving. Also repoints 13 `--color-*` CSS variable references that never resolved under either version (#1255)
  * `grafana/`'s `react` stays on 18: `@grafana/ui@12.4.6` declares `peerDependencies: { react: "^18.0.0" }`, and a datasource plugin shares the host Grafana app's React runtime rather than bundling its own, so React 19 would both violate the peer range and risk two copies of React at runtime. Revisiting is gated on Grafana shipping a React 19 SDK (#1255)
  * Bump the self-hosted CI runner's default `RUNNER_VERSION` in `docker/github-runner.Dockerfile` from 2.332.0 to 2.336.0 (fallback only — `build/dev_worker.py` already pins the latest release at build time when its GitHub API lookup succeeds)
* **Ingestion:**
  * Make block payload object writes create-only instead of an unconditional overwrite, and make dedup on the write path observable (#1465, root cause diagnosed in #1462): a new `BlobStorage::put_if_absent` (`PutMode::Create`) replaces the plain `put` in `insert_block_typed` and in `replication.rs`'s `ingest_payloads`, so a colliding write to an existing block-object key is rejected rather than silently applied — the lake's object keys are write-once and content-addressed, and this enforces that structurally rather than by convention. This closes the #1462 regression: the OTLP logs webhook path re-derived the same `block_id` on every redelivery and previously overwrote the stored object with a freshly-backfilled, differently-timestamped encoding each time. `insert_block_typed` still falls through to the row `INSERT ... ON CONFLICT (block_id) DO NOTHING` on a colliding write (never returns early), so an orphaned object from a prior attempt that died between PUT and INSERT still gets healed. The four (object, row) outcome combinations now each get one log line and one counter instead of a single invisible `debug!`: normal first write stays `debug!` with no counter; object-and-row-both-already-exist (retry, or two distinct events with identical bytes) is `warn!` + `block_object_duplicate`; object-existed-but-row-was-missing (orphan healed, or the losing side of a concurrent-duplicate race) is `warn!` + `block_orphan_object_healed`; row-existed-but-object-was-missing (object lost/deleted, or the winning side of a concurrent-duplicate race) is `debug!` + `block_object_recreated`. Also removes the `observed_time_unix_nano` backfill and the resulting pre/post dual-encode in `split_logs` (`rust/otel-ingestion/src/block.rs`): the stored proto is now byte-identical across retries, so `block_id` (hashed from those bytes) is provably the hash of what's actually stored, and `blocks.payload_size` is provably correct for same-encoding arrivals. The arrival-time fallback this backfill used to provide moves entirely into the block's `begin_time`/`end_time` (already computed by `logs_bounds`/`build_prepared_block`) and, on read, into `OtelLogsBlockProcessor`, which now substitutes the block's `begin_time` for a record with no timestamp of its own instead of dropping it. See `mkdocs/docs/otlp/index.md` (Idempotency), `mkdocs/docs/admin/ingestion.md` (Scaling), and `mkdocs/docs/admin/service-lifecycle.md` (Data durability). **Configuration requirement**: the object store must support conditional put (`PutMode::Create`); an S3-compatible store explicitly configured with `aws_conditional_put=disabled` now fails every block write with an actionable error instead of degrading — this is a deliberate hard failure, not a regression, since silently falling back to overwrite would restore the bug this change fixes. **Deployment note (split mode only)**: deploy `telemetry-maintenance-srv`/analytics before `telemetry-ingestion-srv` during a rolling upgrade. Reversed, a window opens where new zero-timestamp OTLP logs writes are dropped (not substituted) by an old maintenance daemon still running the pre-#1465 processor; monolith mode has no such window. Once the new maintenance/analytics build deploys and rebuilds a partition, any surviving block from the `3f1cf089e` (#1031) → `17bb18505` (#1124) window — which predates the backfill this change removes and so already carries zero-timestamp records — starts surfacing rows it previously dropped, a benign row-count increase, not a hazard.
  * Encode `BlockPayload`'s `dependencies`/`objects` fields as CBOR byte strings instead of one array item per byte, cutting stored block payload size by ~40-45% for new blocks (#1463): a new `rust/telemetry/src/serde_byte_buf.rs` helper switches serialization to `serialize_bytes`, while deserialization stays tolerant of the legacy array-of-integers form permanently, since blocks already in object storage are never rewritten. No client-side change is needed — the Unreal sink has always emitted byte strings, and the Rust `telemetry-sink` and ingestion server both pick up the smaller encoding automatically from this one crate change. Expect `blocks.payload_size` to drop for newly stored blocks, and, via `block_partition_spec.rs`'s `nb_tasks = (100 MB / max_size).clamp(1, 64)` heuristic, a rise in ETL partition-build concurrency (and peak memory) for views whose largest block payload exceeds ~1.56 MB — an accepted, expected side effect, not a regression. **Deployment note**: the create-only block write above (#1465) is the change that unblocks this rollout — with it in place, a redelivered `block_id` can no longer overwrite an existing object with a differently-encoded body, so this no longer waits on #1462.

## v0.28.0 - 2026-08-02

* **Claude Code Plugin:**
  * Fix the `micromegas-query` skill failing to load outright whenever `micromegas-query` isn't installed or its connection isn't configured yet — precisely the two cases its own `## Setup` section exists to repair. Replace the load-time `!`-prefixed shell probes (which aborted the whole skill load on a non-zero exit) with an ordinary first-use check the agent runs and reacts to, and switch persistent configuration from a `~/.micromegas_env` file sourced by the shell profile (which never reached the agent's own subsequent tool calls) to `~/.micromegas/config.json`, already read by the library on every invocation; also tighten `allowed-tools` to the minimum the skill needs (#1404)
* **Python:**
  * **Breaking change**: raise the minimum supported Python version from 3.10 to 3.11 (`pip install micromegas` on 3.10 now resolves to the last 3.10-compatible release instead of picking up new ones). This is what lets `micromegas.time.format_datetime`/the FlightSQL client/`micromegas-query`'s `--begin`/`--end` accept an RFC 3339 `Z`-suffixed timestamp (e.g. `2024-01-01T00:00:00Z`) natively, which the docs already advertised as the canonical spelling but which `datetime.fromisoformat()` only started supporting in 3.11; add a `parse_datetime()` helper in `micromegas/time.py` that also normalizes a lowercase `z` suffix, which the stdlib rejects on every version. Delete the undocumented, drifted-duplicate `flightsql/time.py` in favor of routing the FlightSQL client through the shared `micromegas.time.format_datetime`. `micromegas-query --begin`/`--end` now report a usage error via `argparse` instead of an uncaught traceback on an invalid timestamp, and their help text names RFC 3339. Add a hermetic Python unit-test CI job (`.github/workflows/python.yml` / `build/python_ci.py`) with a 3.11/3.14 matrix and a `black --check` gate (#1405)
* **Web App:**
  * Fix `micromegas-screens`/`micromegas-query` CLI tools mis-decoding non-ASCII text (em dashes, accents, CJK) as mojibake when run under a non-UTF-8 platform locale: pin every text-mode file/stdin read and write to UTF-8 across `screens.py`, `query.py`, and the OIDC token cache, switch pulled screen files and plan/apply diffs to literal UTF-8 instead of `\uXXXX` escapes, and protect `apply`/`pull` from silently deleting or overwriting a screen whose local file can't be decoded or parsed (#1399)
  * Fix folder and screen rows in the Screens sidebar not behaving like real links: ctrl/cmd-click, middle-click, and right-click "open in new tab" now work as expected, since the rows render as actual anchors instead of `div`s intercepting every click (#1394)
  * Recognize UCUM/OTLP unit codes emitted by CloudWatch Metric Streams and other OTLP producers (`By`, `By/s`, `kBy`, `MiBy`, `MBit/s`, `{Count}`, `1`, `Cel`, ...), so a `1234567890 By/s` measure now renders as `1.1 GB/s` instead of a raw number with the code appended; `{...}` annotations are stripped by rule rather than by table entry, every size/bit prefix gains a working `/s` rate form, dimensionless units (`none`, `count`, `{Count}`, `1`) render as bare numbers, and equivalent spellings (`bytes`/`B`/`By`) now share a single Y axis instead of producing one per spelling (#1389)
  * Fix `micromegas-screens` CLI silently dropping a screen's folder assignment on import/pull, and showing a spurious diff that would wipe it on apply: thread `folder_path` through `WebClient.create_screen`/`update_screen` and the CLI's file read/write/diff paths end-to-end (#1362)
  * Add folder organization for saved screens: a `folders` API (list/create/rename/move/delete) backed by a materialized-path `folder_path` column on `screens`, a sidebar folder tree with breadcrumbs and drag-and-drop move, a folder picker in the Save/Save As dialog, and search that also matches folder names; folder rename/move/delete and screen create/move are serialized with Postgres advisory locks to close TOCTOU races between concurrent operations on the same path (#1159)
  * Fix categorical bar charts clipping the first/last bars and off-centering their x-axis labels: pad the categorical x-scale by half a slot via a new `buildXScale` helper so every bar sits fully inside the plot area
  * Add currency-aware value formatting: a metric `unit` recognized as an ISO 4217 currency code (e.g. `USD`, `CAD`, `EUR`) now renders as proper money (`"$1,234.56"`) in tooltips, the stats panel, and Y-axis ticks, instead of falling through to a bare number with the raw unit string appended (#1326)
  * Add an optional per-cell query time range override to every query-backed notebook cell (table, chart, log, property timeline, swimlane, transposed, flame graph, map, expression variables, image, and Perfetto export), so a cell can pin a fixed range or derive it from a variable, an upstream cell result, or a row/drag selection instead of always inheriting the screen's global range; a bad override now surfaces as a cell error uniformly, including on Perfetto export which previously fell back silently (#1314)
  * Add a per-cell "Wrap text" toggle to the notebook Log cell so long or multi-line `msg` values (e.g. stack traces) render wrapped instead of single-line-truncated, defaulting on and persisted in `options.wrapText`; the last column now bounds to the row's available width instead of a hardcoded 700px cap
  * Fix flamechart on-canvas labels bleeding into sibling spans and add ellipsis truncation for long span names (#1305); bound the hover tooltip's width/height, wrap embedded newlines, and position it from its measured size instead of hardcoded offsets (#1306)
* **Analytics:**
  * Fix the maintenance daemon's materialization pass aborting on the first view that fails, starving every view ordered after it for the rest of that pass (and, since the daemon's lookback windows are short, potentially forever): isolate per-view failures in `materialize_all_views` so every view still gets its own materialization attempt, aggregate the failures into a single reported error, and add a `materialize_view_failure` metric tagged by view for observability (#1393)
  * Add order-preserving k-way merge for `SqlBatchView`s with a non-temporal sort key: a new `ScanOrdering::PerFile` scan mode feeds each certified-sorted partition to DataFusion as its own file group so a `SortPreservingMergeExec` collapses them in one pass instead of buffering a full sort, gated by a per-partition `sort_order` degrade check; `SqlBatchView::with_merge_sort_order` lets a view declare its merge sort columns (adopted by `log_stats`), and `regenerate_partitions` can now upgrade partitions materialized before the declaration was added. **Breaking API change**: `View::get_scan_output_ordering` now returns `ScanOrdering` instead of `Vec<ScanSortColumn>`; `PartitionedTableProvider::with_ordering` is renamed to `with_scan_ordering` and now takes a single `scan_ordering: ScanOrdering` argument instead of an ordering `Vec<ScanSortColumn>` plus a separate `OrderingBounds`; `QueryMerger::with_merge_scan_ordering` now takes `ScanOrdering` instead of `Vec<ScanSortColumn>`; `make_partitioned_execution_plan` replaces its `output_ordering: &[ScanSortColumn]`/`ordering_bounds: OrderingBounds` parameters with a single `scan_ordering: &ScanOrdering`; and `fetch_sql_partition_spec` and `SqlPartitionSpec::new` both gain a required `sort_order: Option<Vec<String>>` parameter (#1392)
  * Set `write_partition`'s Parquet `max_row_group_size` to 128 Ki rows for finer row-group pruning (#1392)
  * Materialize OTLP `Summary` metrics (e.g. CloudWatch Metric Streams' `opentelemetry1.0` output) into `measures` as `count`/`sum`/`min`/`max` rows under suffixed metric names, instead of silently dropping them; bump `measures`' schema version so previously-ingested Summary blocks become eligible for re-materialization — note that the bump also retires every existing `measures` partition from query results (partitions are matched on an exact schema hash), so historical metrics stay invisible until `regenerate_partitions('measures', ...)` is run for the desired range; only the maintenance daemon's short trailing windows re-materialize on their own (#1359)
  * Fix a dictionary key overflow panic in the span/async-event/net-span/metrics/log-entries/images table builders and their OTLP companion processors: widen the `Int16`-keyed dictionary columns to `Int32` and replace panicking `append_value`/`append_values` calls with fallible equivalents, so a query batch with more than 32,767 distinct values in one of these columns no longer crashes the background query task (#1341)
  * Make `blocks_view` partition merges order-preserving so merged partitions stay internally sorted by `insert_time`, stream the Postgres-backed partition write path in bounded chunks instead of `fetch_all`-ing a whole insert range into memory, and add a `regenerate_partitions` admin table function to force-regenerate existing merged partitions from source; record a per-partition `sort_order` in `lakehouse_partitions` metadata, and enforce partition insert-time disjointness with a `btree_gist`-backed Postgres exclusion constraint (#1336)
  * Fix Perfetto trace export OOMing on wide-time-range, many-thread processes: declare `thread_spans`' existing scan ordering to DataFusion so `EnforceSorting` drops the per-thread `ORDER BY begin` sort instead of materializing a concurrent `ExternalSorter` per thread against the shared memory pool; `ORDER BY` stays in the query and is still honored, just free (#1297)
  * Hoist the per-segment `blocks_view` partition fetch out of the segment loop in JIT partition generation (`generate_process_jit_partitions`/`generate_stream_jit_partitions`): fetch the partition list once for the whole insert-time range and filter it in memory per segment instead of re-querying Postgres for every 1-hour segment, cutting a multi-day query's redundant round-trips from dozens-to-hundreds down to one, with no behavior change
* **Build:**
  * Bump `brace-expansion` to `^5.0.9` via yarn `resolutions` across the root, `welcome/`, and `analytics-web-app/` workspaces to resolve Dependabot alerts 400-402 (bypasses a prior mitigation for the same DoS advisory), and bump `black` to `^26.3.1` in `python/micromegas`'s dev dependencies to resolve Dependabot alert 399 (arbitrary file write from unsanitized user input in Black's cache file name)
  * Bump the pinned Rust toolchain to 1.97.1, and regenerate the datafusion-wasm bindings whose internal closure-glue symbol names changed under the new compiler
  * Bump `datafusion` from 54.0 to 54.1.0 in both `rust/Cargo.toml` and `rust/datafusion-wasm/Cargo.toml` (bug-fix release; no Arrow version change), and regenerate the checked-in `datafusion-wasm` bindings
  * Bump the root workspace's `resolutions."react-router"` from `^7.18.0` to resolve Dependabot alert 395 (GHSA-qwww-vcr4-c8h2), pinning it via a `yarn patch` descriptor to exactly `8.3.0` (not a `^8.3.0` range — future 8.x patch releases won't be picked up automatically and will need the patch manually regenerated against the new version); the patch neutralizes the one line responsible for react-router 8.x's Jest breakage (`import.meta.hot` in framework-mode `loadRouteModule`, dead code never reached by this app's `react-router-dom-v5-compat`-only usage), and extend `grafana/jest.config.js`'s `transform` to also handle `.mjs` files so the new transitive `cookie-es` dependency parses; also bump `grafana/.nvmrc` and `.github/workflows/grafana-plugin.yml`'s `node-version` from Node 20 to the floating major `22`, and `grafana/package.json`'s `engines.node` from `>=20` to `>=22`, to satisfy `react-router@8.3.0`'s `engines.node >=22.22.0` floor, mirroring `analytics-web-app`'s #1351 precedent; also fix `build/grafana_ci.py`'s `run_cmd` to `nvm use` the explicit resolved Node version instead of a bare `nvm use`, since the bare form silently resolved the nearest `.nvmrc` by walking up from the invoking directory — a bug that would otherwise have silently run most of the Grafana CI suite under Node 20 (the root `.nvmrc`) after this branch's Node 22 bump
  * Bump `react-router-dom` to `react-router@^8.3.0` in `analytics-web-app` (the package was renamed starting with v8), and bump `.nvmrc` from Node 20 to the floating major `22` to satisfy `react-router@8.3.0`'s `engines.node >=22.22.0` floor, to resolve Dependabot alert 388 (GHSA-qwww-vcr4-c8h2); also bump the three Docker frontend-builder stages (`docker/analytics-web.Dockerfile`, `docker/monolith.Dockerfile`, `docker/all-in-one.Dockerfile`) from `node:20-alpine` to `node:22-alpine`, and `docker/github-runner.Dockerfile`'s CI runner image from nodesource `setup_20.x` to `setup_22.x` plus a new nvm install pre-provisioning both Node 20 and 22 (the runner is shared with the Grafana plugin build, which now also pins Node 22)
  * Bump `google.golang.org/grpc` to `1.82.1` in the grafana plugin's Go module to resolve Dependabot alert 381 (GHSA-hrxh-6v49-42gf)
  * Bump `dompurify` to `^3.4.12` (GHSA-c2j3-45gr-mqc4) and `fast-uri` to `^3.1.4` (GHSA-4c8g-83qw-93j6, GHSA-v2hh-gcrm-f6hx) via yarn `resolutions`/`overrides` across the root, `doc/intro-micromegas`, and `doc/high-frequency-observability` workspaces to resolve Dependabot alerts 383, 380, 379, 384, 382
  * Upgrade `react-router-dom` to `^7.18.0` in `analytics-web-app`, and force the transitive `react-router` resolution to `^7.18.0` in the root workspace (pulled in only via `@grafana/ui`'s legacy v5 routing compat shim), to resolve Dependabot alerts 386, 385, 378, 377, 376 (GHSA-wrjc-x8rr-h8h6, GHSA-337j-9hxr-rhxg, GHSA-jjmj-jmhj-qwj2) — no fix exists in the v6 line for these advisories
  * Bump `postcss` to `^8.5.18` via yarn `resolutions`/`overrides` across the root, `welcome`, `analytics-web-app`, `doc/notebooks`, `doc/unified-observability-for-games`, and `doc/high-frequency-observability` workspaces, fixing a path-traversal vulnerability (GHSA-r28c-9q8g-f849) found via `npm audit`
  * Bump the pinned Rust toolchain to 1.97.0; fix new `clippy::useless_borrows_in_formatting` lints the version tightened, and regenerate the datafusion-wasm bindings whose internal closure-glue symbol names changed under the new compiler
  * Pin the transitive `websocket-driver` dev dependency to `^0.7.5` via yarn `resolutions` to resolve Dependabot alerts #339/#340
  * Bump `js-yaml` to `^4.3.0`, `protobufjs` to `^7.6.5`, and `brace-expansion` to `^5.0.7`/`^2.1.2` via yarn `resolutions` across the root, `welcome/`, and `analytics-web-app/` workspaces to resolve Dependabot alerts #341-#347
  * Bump `brace-expansion` to `^5.0.8` via yarn `resolutions` across the root, `welcome/`, and `analytics-web-app/` workspaces (the older 1.x-4.x lines never received a patched release, only 5.x did) to resolve Dependabot alerts 396-398 (GHSA-mh99-v99m-4gvg)
  * Bump `tar` to `^7.5.19` (resolving to 7.5.20) across all yarn workspaces (root, `welcome/`, `analytics-web-app/`, and the `doc/*` sites) to resolve 24 Dependabot alerts (#348-371)
  * Bump `immutable` to `^5.1.8` (resolving to 5.1.9) in the root yarn workspace to resolve 2 Dependabot alerts (#374, #375)
  * Delete the unused `uri-handler` crate, and route `analytics-web-srv` and `object-cache-srv` through the public `micromegas` facade (`micromegas::auth`/`micromegas::object_cache`/`micromegas::tracing`) instead of depending on internal crates directly (#1256)
  * Migrate `analytics-web-app`'s test runner from Jest to Vitest, so tests are parsed by the same engine that builds the app and ESM-only dependencies stop requiring per-package carve-outs; drop the now-unreachable Babel/Jest packages and `handlebars` resolutions pin (#1345)
* **Grafana Plugin:**
  * Upgrade the Grafana plugin SDK from 11.6.7 to 12.4.6: replace the frozen `@grafana/experimental` with its successor `@grafana/plugin-ui`, bump `@grafana/tsconfig`/`@grafana/plugin-e2e`/`@testing-library/react` to satisfy peer dependencies, bump the local/e2e test Grafana instance to 12.4.6, and add e2e coverage for the `SQLEditor` swap (#848)
* **Caching:**
  * Fix `CacheClientStore::full_stream_with_fallback` surfacing the cache's own error (or, latently, silently truncating) on a `GET /obj` body that fails or ends short partway through: it now resumes the remainder from the direct store at the byte offset already delivered, so the only errors a client can ever surface are the direct store's own (#1360)
  * Add a client-side circuit breaker gating `CacheClientStore`'s reads and prefetches: after 5 consecutive unresponsive requests, reads and prefetches skip the cache entirely for a fixed 3s cooldown, with one probe request admitted per cooldown to detect recovery; replace the client's two flat 2s/15s timeouts with a `CacheClientConfig` (50ms connect, 500ms per-request abandon budget, 3s stall budget, 15s unchanged total deadline), all but the fixed budgets overridable via new `MICROMEGAS_OBJECT_CACHE_CLIENT_*` env vars (#1360)
  * Fix a benign but observable write race in `FoyerBackend::get`: replace foyer's `HybridCache::get` with a two-step read (RAM lookup, then direct disk load) whose disk→RAM promotion is gated on a caller-supplied expected length, plus a per-key single-flight around the disk load to keep read-coalescing parity; this makes foyer's inflight decoy-close-flag path unreachable and adds `object_cache_ram_tier_hit`/`object_cache_disk_tier_hit`, `range_cache_load_coalesced`, and `range_cache_promotion_len_mismatch` metrics (#1318)
  * Add `object_cache_promotion_count`/`object_cache_promotion_bytes` (`+ prefix`) metrics measuring disk→RAM promotion volume, emitted alongside `object_cache_disk_tier_hit` at the single validated-promotion crossing point in `FoyerBackend::promote_if_valid`; together with `object_cache_ram_tier_eviction_*` they make RAM-tier churn (promotion in vs. eviction out) directly observable (#1321)
  * Add an `object_cache_ram_tier_entries` saturation gauge alongside `object_cache_ram_tier_usage_bytes`, giving RAM-tier occupancy in entry count as well as bytes (#1322)
  * Fix `object_cache_get_bytes_served` recording a structural zero on the live `GET /obj/{key}` path: a `Content-Length`-framed HTTP body is never polled to a terminal `None`, so the byte-count callback fired from the drained stream never ran; `count_bytes_served` now fires as soon as the known expected payload is fully produced, before the final chunk is yielded, and both GET and `/ranges` call sites pass their known total (#1279)
  * Instrument object-cache eviction: emit RAM-tier eviction count/age metrics (tagged by `prefix` and `reason`) via a foyer `EventListener`, and a disk read-age estimate on disk-tier read hits, so it's observable what gets evicted and how long it lived before leaving a tier (#1281)
  * Fix a demand-fill leak where a cached block stored as a slice of its coalesced origin-GET buffer pinned the whole parent allocation, letting RAM-tier RSS run up to `max_coalesced_get_bytes / block_size`x its accounted weight; copy on demand admission in both `FoyerBackend` and the L1 `BoundedMemoryBackend` to detach the cached block, and export accounted RAM-tier usage as a new `object_cache_ram_tier_usage_bytes` saturation gauge (#1276)
  * Fix the same class of leak on the prefetch path: `FoyerBackend::put`'s prefetch arm stored the incoming slice verbatim, keeping its coalesced origin-GET parent buffer alive for as long as the entry lived in foyer's disk-write pipeline; copy before insert, mirroring the demand arm (#1317)
  * Add an `object_cache_head_requests` (`status`, `prefix`) counter to the `HEAD /obj/{key}` path in `object-cache-srv` by splitting `head_handler` into a thin counting wrapper + inner handler, mirroring the existing GET/`ranges` wrappers, so HEAD traffic volume and its status-code distribution are directly visible instead of only inferable as a residual of other call sites' size/HEAD-tier metrics (#1280)
  * Guard the foyer disk cache against stale on-disk formats: stamp a `DISK_FORMAT_VERSION` marker file in the disk dir and wipe it on a version mismatch or missing marker, so a value-layout change can never misdecode an old entry; add a `MAX_PLAUSIBLE_OBJECT_SIZE` sanity-check to the `size()` fast path (on both the cached-read and origin-HEAD paths) that rejects an implausible size instead of driving a catastrophic allocation, with `object_cache_disk_format_wiped` and `range_cache_size_implausible` metrics (#1287)
  * Fix `object-cache-srv`'s graceful shutdown dropping in-flight origin fetches instead of draining them: the grace period now also aborts the prefetch worker, waits for outstanding detached fetch tasks (coalesced block-run GETs and `size()` HEADs) to finish, and closes the foyer disk cache so a drained fetch's bytes survive a restart, all best-effort within whatever of the grace period axum's own HTTP drain didn't use; also fixes `FulfillGuard`'s shutdown-drop log incorrectly claiming a panic (#1291)
* **Docs:**
  * Fix `schema-reference.md`/`functions-reference.md`/`how_to_query` docs to match the `Int16`->`Int32` dictionary widening above, and correct stale `processes`/`streams` string-column types (Utf8, not `Int16` dictionaries) and a missing `images` table `Int32` list entry (#1341)
  * Rewrite the "Getting Started" page as a Docker-based quickstart (clone-free `curl` + in-repo compose options, self-contained monolith compose file with inline DB init, web app, optional Python sample query, stop/cleanup, troubleshooting); relocate the developer/build setup to the Build Guide and repoint entry-point links; note the Compose v2.23.1+ prerequisite where the shared compose file is invoked (#1273)
  * Split the root `CLAUDE.md` into per-subproject files (`rust/`, `python/`, `analytics-web-app/`, `grafana/`, `mkdocs/`) so subproject-specific guidance only loads when that subproject is touched, trim the root file to cross-cutting rules, and delete the now-fully-absorbed `AI_GUIDELINES.md`
* **Config:**
  * Consolidate ad-hoc `MICROMEGAS_*` env-var reads into typed config structs (`DataLakeConfig`, `WebServerConfig`, `CommonServerArgs`, `GatewayConfig`) and shared helpers (`parse_object_store_url`), replacing ~15 scattered `std::env::var` call sites; flatten `CommonServerArgs` into the service binaries and add an env-backed `--flightsql-url` flag to `http-gateway` to fail fast instead of reading the env per request (#1248)
* **Observability:**
  * Wrap DB query futures and the remaining JIT partition generation / block-write futures (partition cache fetches, DataFusion collects, block streaming, spawned task joins, logger writes) across analytics, ingestion, public, and analytics-web-srv with `instrument_named!`, so each round-trip shows up as its own span nested under the caller, separating DB/IO wait time from CPU time in traces
  * Add process RSS/virtual-size gauges (`process_resident_bytes`/`process_virtual_bytes`, allocator-agnostic via `sysinfo`) and jemalloc runtime stats (`jemalloc_allocated_bytes`/`jemalloc_resident_bytes`/`jemalloc_mapped_bytes`/`jemalloc_retained_bytes`, via a new `jemalloc-metrics` feature) to the shared `system_monitor` sampler every service runs, sampled every ~5s, so a production memory climb can be diagnosed as this process vs. another, a logical leak vs. allocator retention, or heap vs. non-heap growth (#1319)
  * Add a `pg_stat_*` self-observability collector to the maintenance daemon: a `PgStatsTask` samples PostgreSQL's standard `pg_stat_*` views (plus index/table sizes) once a minute and emits the readings as per-relation-tagged `pg_*` micromegas metrics, turning static questions like "which indexes are dead weight" into queryable runtime signals; includes an admin-docs section with sample unused-index and cache-hit-ratio queries (#1292)
  * Emit one structured JSON audit record per FlightSQL query under the `flightsql_query_audit` log target, unifying request attribution (client, user, email, service account, SQL, time range) with per-query cost (stage durations, output rows, bytes scanned) in one queryable row so expensive queries can be attributed to their source; retains the physical plan to read DataFusion metrics post-drain, makes the lakehouse parquet reader record `bytes_scanned` (previously structurally zero), and audits cancelled and setup-failed queries too (#1288)
* **Ingestion:**
  * Add a generic header-described `POST /ingestion/webhook` endpoint: any header-capable webhook producer (GitLab, GitHub, generic SaaS) can report directly to micromegas with no external transformer service, using `X-Micromegas-*` headers to synthesize an OTLP `Resource` and the verbatim request body as a single log record's `msg`, reusing the existing OTLP logs identity/block/write path end-to-end (#1296)
  * Add a `POST /ingestion/otlp/v1/metrics/firehose` endpoint speaking the Amazon Kinesis Data Firehose HTTP Endpoint Delivery protocol, so a CloudWatch Metric Stream (OpenTelemetry 1.0.0 output) can push metrics into micromegas as Metric Stream → Firehose → micromegas with no Lambda, Kinesis Data Stream, or collector in between: a thin envelope adapter unwraps the Firehose JSON/gzip batch, base64-decodes each record, and feeds it into the existing OTLP metrics decode/split/write path; it authenticates the `X-Amz-Firehose-Access-Key` header against the same API-key keyring and returns the Firehose ack response shape so failed batches retry and spill to S3 rather than dropping data (#1299)
  * Fix the still-unreleased Firehose metrics route (#1299) decoding each record as a single unframed protobuf message: CloudWatch Metric Streams actually pack one-or-more length-delimited `ExportMetricsServiceRequest` messages per record, so a batch of more than trivial size failed with errors like `invalid wire type value: 7`; decode and write each message as it's found instead, so a later malformed message can't discard already-decoded, already-written messages earlier in the same record (#1381)
  * Fix the still-unreleased Firehose metrics route (#1299) collapsing every CloudWatch Metric Stream onto one degenerate `process_id`: partition each matching resource into one synthetic `ResourceMetrics` per CloudWatch namespace (`AWS/RDS`, `AWS/ECS`, …, falling back to `AWS/Unknown` when a metric carries no `Namespace` datapoint attribute), with `exe` set to the namespace and `service.instance.id` set to the exporter ARN so different accounts/regions never collide; only newly ingested deliveries get per-namespace `process_id`/`exe` values under this scheme — the rewrite happens at ingestion time, so already-ingested CloudWatch Metric Stream blocks keep their old degenerate identity and re-materialization does not backfill it (#1387)
  * Add a `POST /ingestion/otlp/v1/logs/firehose/cloudwatch` route decoding the CloudWatch Logs subscription-filter delivery format, so RDS/ECS/Lambda logs can reach micromegas as CloudWatch Logs → subscription filter → Firehose → micromegas with no intermediate consumer: each per-record payload (gzip-compressed JSON, one `logGroup`/`logStream`/`owner` plus a batch of `logEvents`) is synthesized into an `ExportLogsServiceRequest` and fed into the existing OTLP logs decode/split/write path unchanged, reusing the Firehose envelope/auth machinery from #1299; per-record and aggregate-per-batch gunzip size caps guard against decompression bombs, and events with a negative timestamp are rejected rather than silently accepted (#1300)
* **Native / Blender:**
  * Add a dev-only **Keep Alive** add-on preference to the Blender extension: on disable the live telemetry session is parked on a `sys` attribute instead of shut down, and the next enable in the same process reuses it (same `session_id`), so the add-on no longer goes inactive on every re-enable — the native layer can only `mm_init` once per process, which tooling that re-enables the add-on on each launch (the VS Code Blender-Dev extension) hits every debug session; a parked session is reused even if the preference is later turned off, since no second one can be created, and the `atexit` flush hook is re-pointed at the current module instance on every `register()` so it stays attached to the namespace holding the live session across a reload that purges `sys.modules`
  * Capture redo-panel (F9) parameter edits in the Blender extension: Blender re-executes the same operator in place — same `wmOperator`, new parameters — and fires `undo_post` rather than `redo_post`, so the operator-history pointer diff saw an already-known entry and the edit was logged as a plain "undo" with the original parameters. The newest entry's formatted message is now baselined whenever the newest entry changes and diffed on `undo_post`/`redo_post`; a mismatch is logged to a new `blender.action_redo` target (and counted in `blender.action_captured`) in place of the "undo"/"redo" lifecycle log, since the underlying fact is a parameter change. Macro operators are skipped — their sub-operator values are unreachable from a stored history entry, so there is nothing to diff — and the baseline is deliberately not refreshed while the newest pointer is unchanged, so a poll landing between the edit and `undo_post` cannot mask it (#1365)

## v0.27.0 - 2026-07-12

* **Analytics:**
  * Move `ThreadSpansView::jit_update` off Postgres onto DataFusion views: replace the unprunable `blocks` full-table scan (TSC frequency estimation) and the `streams`/`processes` PK reads with `find_stream_from_view`/`find_process_with_latest_timing` against the DataFusion `streams`/`processes` views, and delete the now-dead `make_time_converter_from_db`/`find_stream` Postgres helpers (#1244)
* **Tests:**
  * Delete the orphaned, never-compiled `rust/http-gateway/src/config.rs` (duplicate of `HeaderForwardingConfig` in `public::servers::http_gateway`) and its dead tests; move three `tracing` crate inline `#[cfg(test)]` modules (`time.rs`, `logs/events.rs`, `string_id.rs`) into the crate's `tests/` folder per convention; mark `ingestion`'s `readiness.rs` live-dependency test `#[ignore]` instead of silently no-op-passing without a DB; replace a fixed 50ms sleep in `large_message_tests.rs` with a bounded TCP readiness poll (#1219)
  * Tighten timing/sleep-based tests: replace a meaningless fixed sleep in `cron_loop_drains` with a task-started signal, swap the random sleeps in `async_span_tests.rs` for fixed 1ms ones (dropping the `rand` dev-dependency), and correct the `thread_park_test` / `TracingRuntimeExt` docs to describe flush-on-thread-stop rather than a nonexistent `on_thread_park` callback (#1252)
* **Tracing:**
  * Harden the Rust telemetry sink's HTTP transport: priority queues with graded byte-budget dropping, lazy stream metadata (no `insert_stream` for idle/short-lived streams), per-priority retry tuning, and in-flight request gating with concurrent sends, porting the Unreal sink's resilience under backpressure and network flakiness to Rust (#1217)
* **Caching:**
  * Add a standalone range-aware S3 read cache (`micromegas-object-cache-srv`) backed by local SSD (Foyer RAM+disk): new `micromegas-object-cache` crate (cache engine + client `ObjectStore` layer) and `object-cache-srv` HTTP binary, wired into `BlobStorage`; clients fall back to the direct store on miss/error. Adds an `object-cache.Dockerfile`, MinIO-backed local test scripts, and admin docs (#1122)
  * Store `FoyerBackend` cache values as `Bytes` instead of `Vec<u8>` to avoid full-block copies on every RAM-tier read hit and fill (#1195)
  * Accept a list of allowed key prefixes in `object-cache-srv` (comma-separated `MICROMEGAS_OBJECT_CACHE_PREFIX` or repeated `--prefix`) so it can serve both `blobs/` and `views/`; the server now fails closed with no prefixes configured, requiring an explicit `--allow-all-prefixes` dev opt-out (#1204)
  * Install a byte weighter on `FoyerBackend`'s RAM tier so `ram_mb` bounds resident bytes instead of entry count, fixing an OOM risk under sustained load (#1207)
  * Rework the `object-cache` read path: single-flight fetch coalescing, a priority budget that lets demand reads jump ahead of prefetch, and a memory bound on in-flight prefetch; fixes a panic-leaking-a-held-permit bug, a batch-promotion race, and a permit undercount found in review (#1203)
  * Add `POST /prefetch` to `object-cache-srv` plus a `CacheClientStore::prefetch` client method, activating the prefetch-priority fill path from #1203: a bounded, load-shedding queue drives fills through the existing scheduler and returns `202 Accepted` immediately; prefetch fills are now admitted to the SSD tier only (never RAM), and a hit-path block-length guard heals cache entries poisoned by an undersized caller-supplied object size (#1198)
  * Stream `POST /prefetch` ingestion as NDJSON (one `PrefetchItem` per line) instead of buffering a whole JSON body, removing the request-body size ceiling: items are parsed incrementally and enqueued as they arrive, with a per-line cap the only remaining bound; deletes the `PrefetchRequest` wrapper type (#1218)
  * Stream `/ranges` and single-range GET responses from `object-cache-srv` over a bounded per-request window instead of assembling the whole response in memory: removes the 512 MiB `MAX_TOTAL_REQUESTED_BYTES` cap (and its `413`), reimplements `get_range`/`get_ranges` as collectors over a new `stream_ranges`, charges memory permits proportionally (capped at the window), adds a startup floor on `--memory-budget-mb` so a small budget fails fast instead of hanging, and adds a mid-stream direct-store fallback to the full-GET client path (#1222)
  * Warm the object cache at write time: once a freshly-materialized Parquet partition is durable in the origin store and committed to `lakehouse_partitions`, `write_partition_from_rows` fire-and-forget POSTs its key to `/prefetch` so the follow-up query's first read is a cache hit instead of a cold origin GET; adds a `PrefixPrefetch` adapter so warm keys match `PrefixStore`-derived read keys, surfaces the cache client's prefetch face and lake root on `DataLakeConnection` as a general `warm_object` primitive, and adds the `object_warm_requested` metric (#1201)
  * Add object-cache performance telemetry to locate bottlenecks and tune: per-stage latency spans + duration metrics (origin GET, backend read, fetch-permit wait), `prefix`/`class` dimensions on the hit-rate counters, and a saturation-monitor sampler for fetch-budget/mem-budget/prefetch-queue occupancy plus host NIC/SSD throughput; also counts all GET/`/ranges` outcomes (not just success) with a `status` dimension and fixes a double-counted `range_cache_size_backend_hit` on ranged GETs via new `*_with_size` read variants (#1206)
  * Make the foyer disk-cache write path tunable and observable: upgrade foyer 0.14 → 0.22, add `MICROMEGAS_OBJECT_CACHE_FLUSHERS` (default 2) and `MICROMEGAS_OBJECT_CACHE_WRITE_BUFFER_MB` (default 128) to size the flushers and write-buffer pool (submit-queue threshold pinned to 2× the buffer) so prefetch bursts stop overflowing foyer's submit queue, and emit `object_cache_foyer_disk_*` throughput gauges sourced from foyer's own `Statistics`, replacing the sysinfo-based `object_cache_ssd_*` gauges that read 0 in the deployed container (#1228)
  * Remove the postgres `partition_metadata` table (schema v6, `upgrade_v5_to_v6`): partition Parquet metadata is now read solely from the Parquet footer via the object-cache-backed reader, with the write-path `INSERT INTO partition_metadata` and the cleanup-path batch delete removed — fixing TOAST overhead on partition-retirement deletes and write-path overhead on every partition insert (#1121)
  * Add an in-process L1 read cache installed on the object stores DataFusion reads through (parquet partitions and static tables), caching hot row-group bytes for files of all sizes via the `object-cache` `RangeCache` core over a shared byte-bounded RAM backend (`MICROMEGAS_OBJECT_CACHE_L1_MB`, default 200, 0 disables); removes the old whole-file `FileCache`/`CachingReader` (and its `MICROMEGAS_FILE_CACHE_MB`/`MICROMEGAS_FILE_CACHE_MAX_FILE_MB` knobs) that only cached files ≤10 MB, and excludes raw-blob reads since those go through `BlobStorage` on a separate store reference (#1205)
  * Refactor `object-cache`'s 1290-line `range_cache.rs` into an `error`/`scheduler`/`fetch` submodule split and decompose the ~294-line `fetch_blocks` into cohesive helpers (`probe_blocks`, `register_missing`, `spawn_run_fetch`, `join_demand`/`join_prefetch`); pure behavior-preserving refactor with the public API and metrics unchanged (#1250)
  * Fix a lost-wakeup deadlock in `range_cache::InFlight::fulfill`: `watch::Sender::send` drops the result when the channel has zero receivers, and joiners subscribe lazily inside `join()`, so a fetch completing before any joiner subscribed would hang every later joiner forever; switch to `send_replace`, which stores the value unconditionally (#1259)
* **OTLP:**
  * Populate the `processes.username` column from `process.owner` (falling back through `process.user.name`, `process.real_user.name`, then `user.name`) and fold the resolved owner into the `process_id` derivation so processes that differ only by owning user get distinct ids; re-derivation stays under the existing `NS_OTEL_PROCESS_V1` namespace
* **Auth:**
  * Consolidate OIDC login-flow client construction into the `micromegas-auth` crate (new `oidc_client` module with a `DiscoveredProvider` that owns provider discovery + client building) so it lives in one place instead of being reimplemented in `analytics-web-srv`, and split the 909-line `analytics-web-srv/src/auth.rs` into focused `config`/`state`/`cookies`/`claims`/`handlers` submodules; behavior-preserving, public API unchanged (#1249)
* **Security:**
  * Gate the five mutating lakehouse SQL functions (`retire_partitions`, `materialize_partitions`, `regenerate_partitions`, `retire_partition_by_file`, `retire_partition_by_metadata`) on the authenticated caller's admin status: thread `is_admin` across the tower `AuthService` gRPC boundary via a new `x-auth-is-admin` header (mirroring the existing `x-auth-subject`/`x-allow-delegation` pattern), and only register these functions on a `SessionContext` when the caller is an admin. **Breaking behavior change**: any authenticated FlightSQL caller — including static API keys, which can never be admin — previously could call these functions; now only OIDC callers matched against `MICROMEGAS_ADMINS`/`MICROMEGAS_ANALYTICS_ADMINS` can, and everyone else gets "function not found". `--disable-auth` (local dev/monolith) continues to treat every caller as admin. **Breaking API change**: `register_lakehouse_functions`, `register_functions`, `make_session_context`, and `query` in the published `micromegas` crate (`rust/analytics/src/lakehouse/query.rs`) all gain a required `is_admin: bool` parameter (#1377)
  * Fix Dependabot alerts: bump `golang.org/x/net` to 0.55.0 in the Grafana plugin backend and `joserfc` to 1.7.2 in the Python client (#338, #337)
  * Upgrade `opentelemetry-proto` to 0.32 to resolve GHSA-w9wp-h8wv-79jx; treat the new profiling-only string-interning fields (`*_strindex`) as absent on non-profiling OTLP signals, per the OTLP spec (#336)
  * Harden the transit block parsing path (`parser.rs`, `dyn_string.rs`, `serialize.rs`, `parsing.rs`) against malformed/truncated payloads: replace unchecked arithmetic, slicing, and raw-pointer reads with checked variants that return `Err` instead of panicking or triggering UB; add a choke-point error log on parse failure and extensive corrupt-input regression tests (#1192)
* **Build:**
  * Add pre-merge supply-chain gates to Rust CI: `cargo audit` (RustSec advisory scan) and `cargo deny check licenses bans sources` (license/bans/sources policy), run against both the main workspace and the excluded `datafusion-wasm` tree; bump `crossbeam-epoch` and `quinn-proto` to clear advisories and document the remaining `rsa`/`quick-xml` ignores (#1246)
  * Add `dev_worker.py --size` to report the runner container and cache volume sizes, and make `--cleanup` also delete the cache volume
  * Fix `build_blender_plugin.py` artifact lookup when `CARGO_TARGET_DIR` is set; add `x86_64-pc-windows-gnu` to the pinned toolchain so the Windows cross-target installs automatically on fresh checkouts
  * Add Docker Hub publish pipeline for release: `build_docker_images.py` gains an arm64 buildx `--push` path and an `--all-arches` flag that builds both architectures in one run, with `--push` independently controlling publishing; sync the release runbook with a both-arch Docker Images phase (#1165)
  * Make `build.py` the single canonical generator for committed datafusion-wasm bindings; prune known wasm-pack leftovers from the output dir after each build so the copy loop is self-healing; document that `wasm-pack build` must not be run into the output dir (#1169)
  * Fix `capi-release`: route both build legs through the dev-worker runner (mold + mingw-w64 in the image), cross-compile the Windows artifacts on the Linux runner, and add the cross target to the pinned `1.96.0` toolchain the build actually uses (#1175)
* **Dependencies:**
  * Update DataFusion to 54.0 and rebuild the datafusion-wasm bindings
  * Migrate the internal proc-macro crates (`micromegas-proc-macros`, `micromegas-tracing-proc-macros`, `micromegas-derive-transit`) from `syn` 1.0 to 2.0 and bump the workspace pin, dropping the duplicate `syn 1.0.109` from the build; macro behavior unchanged (#1253)
* **Native / Blender:**
  * Add `micromegas-capi` C ABI crate (`cdylib` + `staticlib`) exposing init/shutdown, log, and int/float metric FFI over the existing Rust telemetry producer stack; ships a hand-authored C header and 8 smoke tests (#1160)
  * Add Blender Python extension (`blender/micromegas_blender/`) in Blender 4.2+ Extension format with `blender_manifest.toml`; captures user actions (modal recorder, `bpy.msgbus`, `bpy.app.handlers`), performance metrics, and process fingerprint via ctypes binding to `libmicromegas_capi` (#1160)
  * Add crash harvester: on startup scans for prior `*.crash.txt` files, claims each atomically via rename to prevent duplicate uploads, and ships as a CRITICAL log keyed to the prior session fingerprint (#1160)
  * Add `blender-extension` CI workflow: builds the cdylib for Linux and Windows, packages the extension zip, and uploads artifacts; add `capi-release` workflow: builds and publishes versioned `micromegas-capi` binaries (#1160)
  * Add docs for native C ABI integration (`mkdocs/docs/native/`) and the Blender add-on (`mkdocs/docs/blender/`) (#1160)
  * Make Blender per-process memory cross-platform and periodically sampled, and fix the misleading `blender.eval_ms` metric (renamed, no longer spans the file-load boundary) (#1168)
  * Expand the Blender process fingerprint and add Python exception capture via `sys.excepthook` plus semantic user-action capture, closing root-cause-analysis telemetry gaps (#1168)
  * Log add-on version and git commit hash on registration for easier triaging of field reports
  * Fix operator-history overflow gaps: drain the ring on every discrete input event via the recorder modal callback (per-keystroke cadence), lower the timer backstop from 1 s to 0.1 s, and set ring capacity to the correct Blender hard-cap of 32; adds `blender.action_gap` and `blender.action_captured` metrics (#1181)
  * Replace positional `bl_idname` string-diffing of the operator-history ring with per-entry object identity (`op.as_pointer()`) tracking, making new-entry detection exact and eliminating the false-gap WARN and duplicate action-log storm on repeating operator histories; gap detection reduces to a true full-ring turnover (#1181)
* **Docs:**
  * Add a caching architecture page describing the tiered L1/L2 object read path, and show the tiered read cache in the overview architecture diagram (#1238)
  * Add per-service admin guides for the ingestion, FlightSQL, and maintenance-daemon services
  * Refocus the object-cache admin guide on configuration, correcting the cache-warming and cache-client framing
  * Fill README gaps (web app, maintenance daemon, C ABI/Blender, monolith, cache) and fix code-verified inaccuracies across the docs (schema/config references, removed UDFs, binary names)

## June 2026 - v0.26.0

* **Security:**
  * Fix 24 Dependabot alerts: bump undici to ≥6.27.0 (resolves to 8.5.0) across all yarn workspaces (analytics-web-app, welcome, doc/intro-micromegas, doc/notebooks, doc/unified-observability-for-games, root) (#335, #334, #333, #332, #331, #330, #329, #328, #327, #326, #325, #324, #323, #322, #321, #320, #319, #318, #317, #316, #315, #314, #313, #312)
  * Fix 20 Dependabot alerts: bump protobufjs (≥7.6.3), tar (7.5.16), js-yaml (4.2.0), @babel/core (≥7.29.6), ws (8.21.0) across all npm workspaces; bump vite (≥7.3.5) in doc workspaces; bump cryptography (49.0.0) in Python; bump chi (5.2.4) in Grafana backend (#311, #310, #309, #308, #307, #306, #305, #304, #303, #302, #301, #300, #299, #298, #297, #296, #295, #294, #293, #292)
* **Tracing:**
  * Extend `#[micromegas_main]` with optional arguments (`ctrlc_handling`, `local_sink_enabled`, `local_sink_max_level`, `install_log_capture`, `system_metrics`, `telemetry_url`, `api_key`) so callers can configure the `TelemetryGuardBuilder` inline; invalid arguments now emit span-anchored `compile_error!` diagnostics instead of panicking
  * Add image streams: instrumented applications can send screenshots or other images as telemetry via `send_image()`; images are queryable via the `images` SQL table with a `data Binary` column holding raw bytes
* **Unreal:**
  * Add image stream support to the Unreal telemetry sink: wire `ImageStream`/`ImageBlock` into `Dispatch`, `EventSink`, and `HttpEventSink`; add `telemetry.screenshot` console command that captures the game viewport (or editor viewport via `WITH_EDITOR`) as a PNG and sends it as a telemetry image; gate capture with `telemetry.images.enable` CVar
  * Replace manual HTTP retry with `FHttpRetrySystem` (exponential backoff, per-priority retry budgets); add four priority queues (Metadata/Logs/Metrics/Traces) with configurable soft/hard byte-cap drop logic; gate concurrent uploads via `telemetry.max_in_flight_requests` CVar (#56, #43)
  * Add idle-aware spike sampling: suppress spike recording after `telemetry.sampling.interaction_timeout` seconds of no user input; add periodic heartbeat captures every `telemetry.sampling.heartbeat_interval` seconds of active time
  * Emit `TimeSinceLastInput` metric from `FSlateApplication` each frame, with a `BootTime` fallback so the value reads from boot instead of full uptime when no input has occurred yet
  * Replace `volatile` members in `HttpEventSink` with `std::atomic`; remove `QueueSize` and `RequestShutdown` volatile fields (#43)
  * Defer stream-metadata HTTP requests until the first block is enqueued so streams that never produce data generate no spurious uploads; fix `CurrentWorld` weak-ptr to survive same-name map reloads (PIE restart, same-level travel)
  * Emit per-tick camera and player metrics on non-dedicated-server builds: player position (X/Y/Z cm), camera position (X/Y/Z cm), camera orientation (Pitch/Yaw/Roll deg, FOV deg)
* **Deployment:**
  * Retire `telemetry-admin`'s four one-off subcommands (`materialize-partitions`, `retire-partitions`, `delete-old-data`, `delete-expired-temp` — superseded by the `materialize_partitions()`/`retire_partitions()` SQL functions and the daemon's own automatic hourly retention task) and rename the crate/binary to `telemetry-maintenance-srv`, dropping the now-single-mode `crond` subcommand so the binary just runs the daemon; the Docker image is renamed `micromegas-admin` → `micromegas-maintenance` (**breaking change for deployments** pulling the published image). Adds a configurable retention horizon via `--retention-days` / `MICROMEGAS_RETENTION_DAYS` (default 90, threaded through both the standalone daemon and the monolith's maintenance role) to replace the manual `delete-old-data` horizon. Note: the public crate's `daemon()` signature changed with a new `retention_days` parameter (#1266)
  * Add `linux/arm64` cross-compilation support to all production Dockerfiles and build script; builder stages pin to `$BUILDPLATFORM` and install the `aarch64-linux-gnu` cross toolchain so ARM64 images build natively without QEMU; `build_docker_images.py` gains an `--arm64` flag that drives `docker buildx build --platform linux/arm64 --load`; inline the wasm-builder stage to avoid buildx image-store isolation issues
  * Add `micromegas-monolith` single-process binary that runs ingestion, FlightSQL, maintenance, and web in one Tokio runtime sharing a single data-lake connection; includes Docker image, docker-compose stack, `--monolith` start-script mode, per-role auth, and role selection via `--roles` / `MICROMEGAS_MONOLITH_ROLES` (#1139)
* **Performance:**
  * Switch all production service binaries to jemalloc (`tikv-jemallocator`) as the global allocator; reduces allocation latency and memory fragmentation under multi-threaded workloads (#1129)
* **Services:**
  * Add deep `/ready` readiness probe to `telemetry-ingestion-srv`, `flight-sql-srv` (via sidecar HTTP listener on `--health-listen-addr`), and `analytics-web-srv`; each probe verifies its hard dependencies (PostgreSQL pool ping, blob storage list) with a 1s success cache and returns 503 when any dependency is unhealthy so ALBs can drain individual bad tasks during Aurora failover or object-store outages (#1038)
  * Add SIGTERM-driven graceful shutdown to `telemetry-ingestion-srv`, `flight-sql-srv`, `analytics-web-srv`, and `telemetry-admin crond`; in-flight requests, queries, and cron tasks drain within a configurable grace period (default 25s, `--shutdown-grace-period-seconds` or `MICROMEGAS_SHUTDOWN_GRACE_PERIOD_SECONDS`) instead of being killed on ECS task replacement. Note: the `daemon()` and `run_tasks_forever` signatures in the public crate changed (#1037)
  * Add optional `color` column to swimlane cells; per-segment colors support packed RGBA u32, `#rrggbb`/`#rrggbbaa` strings, and 4-byte binary values; falls back to the default theme color when absent (#1127)
  * Add optional `label` column to swimlane cells; labels render as truncated text inside each bar and appear in a hover tooltip alongside the lane name and time range
  * Route `formatDuration` through `formatTimeValue` so flamegraph tooltips show minutes, hours, and days for long spans instead of capping at seconds
  * Add per-row colors, user-selectable series colors, and reference line threshold indicators to XYChart; reference lines support named labels, units, dashed/solid style, and per-line color (#1043)
  * Accept orthographic GLB cameras in the map renderer; GLBs with an embedded `OrthographicCamera` no longer trigger the contract-error banner (#1145)
  * Add a per-Map-cell **Camera** setting with `perspective` (default) and `orthographic` modes; orthographic fits the camera to the projected map silhouette and maps Q/E to zoom (#1065)
  * Add a hover tooltip preview for map markers that renders the cell's `detailTemplate` as a small floating panel following the cursor, with a per-cell show/hide option (#1080)
  * Add `format_value(value, unit)` template function for adaptive unit formatting in Markdown templates (Map detail panel, Markdown cells, and table column overrides); surface unresolved-arg and unresolved-macro warnings via a banner (Map/Markdown) or column-header icon (tables) (#1086)
  * Rework map cell keyboard controls onto a single camera-relative orthonormal basis (A/D strafe, W/S up/down, Q/E forward/back) so key pairs no longer collapse onto the same direction at high camera tilt; radial zoom stays on Ctrl+wheel
  * Fix table column-override memo keying on fresh-per-render objects, causing `evaluateTemplate` to re-run every render; key on a content hash of only template-referenced inputs instead (#1092)
  * Route Map detail-panel `$column` macros through the evaluator's raw row + column-types channel so bare references carry their Arrow `DataType`; timestamps format as RFC3339 and `format_value()` receives full-precision raw values instead of pre-stringified ones (#1091)
  * Size non-fixed log columns (including `msg`) to their longest formatted value on the current page, capped and truncated with full text on hover; `msg` is no longer a special case
  * Add resizable columns to the log cell via draggable inline dividers; drag to pin a column width, right-click a divider to reset to auto or reset all, "Reset widths" button in bottom bar; pinned widths persist in cell options (#1130)
  * Add one-click copy icon to log rows; appears on hover, copies tab-separated row text to clipboard, briefly shows a green checkmark on success (#1131)
  * Add an `image` notebook cell that queries the `images` view and displays results as a navigable carousel; the `format` column is used as the image MIME type and decode failures surface a meaningful error message
  * Fix the data source selector silently rewriting persisted notebook config during render; out-of-scope `$var` references and deleted sources now display as-is (marked unavailable) instead of being switched to the default
  * Fix notebooks losing their `$var` data source when a cell is edited inside a group; group-sibling datasource variables are now valid selector options
* **Analytics:**
  * Accept runtime scalar expressions (CTEs, subqueries, CROSS JOIN columns) as `make_histogram` bounds; literal bounds continue to validate eagerly; NULL histogram rows propagate correctly through all consumers (#1135)
  * Replace SELECT+DELETE pairs in `delete_expired_blocks_batch` with atomic `DELETE … RETURNING` queries; eliminates double-counting and phantom deletes under concurrent writers (#1116)
  * Batch `retire_expired_partitions` to bound memory and transaction size on large backlogs; uses `DELETE … RETURNING` with a row-tuple subquery, matching the established batch pattern (#1111)
  * Prevent phantom empty partitions and detached writer tasks when span builds fail (crossing spans, net-span tree errors, merge stream errors); poison the write channel so `insert_partition` is skipped and the original error propagates to the query; surface full anyhow error chain at the DataFusion boundary
  * Batch `delete_expired_temporary_files` to avoid unbounded SQL/S3 operations on large `temporary_files` tables; add per-file `debug!`-level audit logging (#1108, #1109)
* **OTLP Ingestion:**
  * Backfill `observed_time_unix_nano` at ingestion time for OTLP log records where both timestamp fields are zero; block ID is derived from the pre-mutation bytes to preserve retry idempotency (#1123)
  * Accept `Content-Type: application/json` on all three OTLP/HTTP routes in addition to `application/x-protobuf`; response encoding mirrors the request; enables AWS EventBridge API Destinations to POST directly without a Lambda translation layer (#1115)
* **Build:**
  * Fix `cargo doc --workspace --all-features` hiding the `micromegas-perfetto` library modules by re-keying proto regeneration on the `MICROMEGAS_REGEN_PROTOS` env var instead of the `protogen` feature (#1079)
* **Refactoring:**
  * Remove unused map cell back-compat for the legacy `/maps/` `mapUrl` prefix and `markerColor`/`markerSize` scalar fallbacks; the feature is new so no saved config carries these shapes (#1077)
  * Split four high-complexity web-app files (PerformanceAnalysisPage, FlameGraphCell, MapViewer, XYChart) into pure-logic modules, THREE.js scene helpers, and thin React shells, with unit tests for the extracted logic; behavior unchanged (#1089)
  * Unify template and SQL macro value lookup behind a shared `resolveMacro` helper so both engines route every shape (`time`, `cellRow`, `selected`, `rowCol`, `varCol`, `var`) through one lookup; no behavior change (#1088)
* **Security:**
  * Bump react-router to 6.30.4 and @remix-run/router to 1.23.3 to fix open redirect CVE
  * Bump qs to 6.15.2 and js-cookie to 3.0.7 to fix Dependabot alerts (#256, #257)
  * Upgrade `dompurify` to 3.4.11 (prototype pollution CVE) and `@opentelemetry/core` to 2.8.0
* **Dependencies:**
  * Update DataFusion to 53.1 and rebuild the datafusion-wasm bindings (#1090)

## May 2026 - v0.25.0

* **HTTP Gateway:**
  * Add `GET /gateway/health` liveness endpoint for load balancer probes (#994)
* **OTLP Ingestion:**
  * Add native OTLP/HTTP ingestion for logs, metrics, and traces at `/ingestion/otlp/v1/{logs,metrics,traces}`; resource → `process_id` synthesis via stable UUIDv5; per-block format dispatch on a new `streams.format` column (schema v4)
  * Add `otel_logs_block_processor` (→ `log_entries`) and `otel_metrics_block_processor` (Sum/Gauge → `measures`)
  * Add `otel_spans` JIT view (per-process) materializing OTel spans with `trace_id`/`span_id` as `FixedSizeBinary`
* **Analytics:**
  * Add `net_spans` JIT view materializing Connection/Object/Property/RPC bandwidth spans with cumulative bit offsets
  * Add `rgba(r, g, b, a)` and `lerp_color(c1, c2, t)` scalar UDFs for building packed RGBA `u32` colors from SQL (#1062)
  * Add `color_scale(name, t, alpha)` scalar UDF for sampling built-in perceptually-uniform color scales (viridis, magma, plasma, inferno, cividis, turbo); returns packed RGBA `u32` and replaces the blue→red `lerp_color` pattern with one accessible call (#1069)
  * Add `bin_center(coord, cell_size)` scalar UDF for snapping coordinates to the centers of zero-centered bins; composes into 2D heatmap grids via `GROUP BY bin_center(x, cs), bin_center(y, cs)` (#1068)
  * Add `lerp(a, b, t)` and `unlerp(a, b, x)` scalar math UDFs for 1D remapping; `lerp(c, d, unlerp(a, b, x))` is the canonical `[a,b] → [c,d]` remap (#1083)
* **Web App:**
  * Extend flame graph cell to render bit-axis spans for `net_spans` and add bit-unit support to XYChart
  * Apply adaptive scaling to `bits/s` and `bytes/s` chart axes
  * Fix multi-series line chart rendering only points when series have sparsely-aligned X values
  * Resolve macros in chart series labels and chart cell titles
  * Defer notebook markdown cell render until sequential execution reaches it, preventing stale macro output on first paint (#1023)
  * Add map notebook cell rendering spatial events on a GLB model (#1033)
  * Convert map cell to native UE coordinates (Z-up) with GLB-embedded perspective camera, `MM_ambient_light`, and Neutral tone mapping; replace "Fit to data" with GLB-camera-seeded Reset (#1036)
  * Polish map interaction: cursor-anchored wheel zoom, right-mouse-drag orbit re-anchor, fix marker overlay visibility/picking, surface GLB contract errors in-cell (#1036)
  * Remove map cell `groundSnap` and `heightOffset` options; markers render at their native `(x, y, z)` coordinates
  * Simplify map navigation: WASD flies on hover (no right-click hold), Z resets view; remove right-scroll speed control, Q/E vertical movement, Shift boost, middle-mouse pan, and the speed/event-count overlays
  * Add `--remote-backend` flag to `start_analytics_web.py` for hybrid local-frontend + remote-backend setup (#1033)
  * Silence jsdom `navigation not implemented` console.error in AuthGuard test (#1047)
  * Upgrade to React 19.2, `@react-three/fiber` 9, `@react-three/drei` 10, `@testing-library/react` 16, and `lucide-react` 1.x; drop now-unused `react-reconciler` resolution (#1034)
  * Gate notebook auto-execution on the WASM engine being ready, eliminating an abort race that surfaced as an unhandled DOMException in Firefox (#1034)
  * Pin map `LoadingIndicator` to viewport center so drei's `<Html>` doesn't write a `NaN` transform before the camera is initialized (#1034)
  * Silence upstream `THREE.Clock`, `MouseEvent.mozPressure`, and `MouseEvent.mozInputSource` deprecation warnings emitted by R3F 9 / three 0.183 in dev (#1034)
  * Move map GLB assets and catalog off the static `ServeDir` to an object-store-backed `/api/maps/{catalog,blob}` (cookie-auth-gated, streaming `Body::from_stream`, pass-through `Content-Encoding`); `MICROMEGAS_MAPS_OBJECT_STORE_URI` configures the prefix, `start_analytics_web.py` derives a sane local-dev default
  * Add admin-only Maps management UI at `/admin/maps` with upload (server-side gzip, `.gz`-suffix storage), delete, drag-and-drop, and replace-confirm flow; per-route 256 MiB upload cap configurable via `MICROMEGAS_MAPS_MAX_UPLOAD_BYTES`; scope GLB load failures to the map cell with a retry affordance (#1050)
  * Replace map cell's hard-coded event detail panel with an authorable Markdown template rendered through macro substitution; relax the query contract so `time` is optional and every column is addressable as `$column`; publish map-cell selection to `cellSelections` for cross-cell `$mapcell.selected.col` references (#1053)
  * Refactor map cell to keep SQL results in Arrow Table format through the render path: replace eager `MapEvent[]` materialization with an `Overlay` struct (positions `Float32Array` + table) and on-demand row materialization; split InstancedMesh layout from selection/hover diff so selection changes touch O(1) instances; reject non-finite x/y/z at build time to avoid `InstancedMesh` bounding-sphere poisoning (#1035)
  * Generalize map cell to primitive overlays with shape dispatch (sphere/box), per-instance RGBA via an `instanceColorRGBA` attribute, and column-or-scalar bindings per visual channel (size, scaleX/Y/Z, color); color column accepts integer (packed RGBA u32), `#rrggbb[aa]` string, or 4-byte binary (DataFusion's `0xrrggbbaa` literal); fix SyntaxEditor cursor/overlay alignment in SQL mode (#1055)
  * Enlarge SQL query editors across the app: screen-level editors to 384px and per-cell editors to 240px; trim the flame chart cell description for new-cell dialog consistency
  * Refine map navigation: WASD pans horizontally (no elevation), add Q/E keyboard zoom, suppress the browser context menu when a right-drag releases off-canvas, and auto-size the event detail panel with the title rendered via the Markdown template
  * Remove admin-oriented `MICROMEGAS_MAPS_OBJECT_STORE_URI` / `.glb` drop hint from the map cell editor (admins use the Maps management UI)
  * Resolve `$var` macros in map cell primitive scalar bindings (size, scaleX/Y/Z, color); editor swaps the legacy number/color inputs for a text field with local-draft commit (Enter/blur commits, Escape cancels)
  * Require Ctrl/Cmd modifier for map cell wheel zoom so plain wheel scrolls the surrounding notebook page
  * Fix map cell event detail popup close button overlapping the first line of the markdown template (#1067)
  * Eliminate map cell first-paint camera snap by gating marker/camera mount until the GLB payload has propagated and promoting payload-extract/camera-seed effects to `useLayoutEffect`; restore cursor on controller/marker unmount; rename `UnrealCameraController` to `MapCameraController` (#1075)
* **Python:**
  * Switch `bulk_ingest` to accept `pyarrow.Table` directly for native pass-through of struct/list/binary columns
  * Resolve CLI connection settings from `~/.micromegas/config.json` with env-var override (#1033)
* **Unreal Engine:**
  * Add net trace support with connection/object/property/RPC scopes, runtime verbosity gating, and empty-scope elision
  * Enable crash reporting on Linux and skip telemetry flush during malloc-crash to avoid deadlock
* **Docs:**
  * Add Unreal net trace instrumentation guide and engine recipe
  * Document flame graph notebook cell type
  * Document `net_spans` view in schema reference and network-tracing guide
  * Add "An Introduction to Micromegas" presentation
  * Document map notebook cell type and hybrid local-frontend setup (#1033)
  * Document CLI configuration file and authentication settings (#1033)
  * Document `rgba` and `lerp_color` color functions in the SQL functions reference (#1062)
  * Document `color_scale` perceptual colormap function in the SQL functions reference (#1069)
  * Document `bin_center` binning function in the SQL functions reference (#1068)
  * Document `lerp` and `unlerp` math functions in the SQL functions reference (#1083)
  * Rework notebook cell-types reference: alphabetize sections, expand Map cell with channel mapping and color encodings, fact-check defaults/levels/bindings against the implementation, and move admin/maps content to the Admin → Web App page
* **Security:**
  * Bump rustls-webpki, rand, and uuid to fix Dependabot alerts (#210-213)
  * Bump postcss and uuid to fix Dependabot alerts (#214-221)
  * Bump urllib3, fast-uri, @babel/plugin-transform-modules-systemjs, and apache/thrift to fix Dependabot alerts (#222, #225-229)
  * Bump mermaid to 11.15.0 in `doc/intro-micromegas` and `doc/high-frequency-observability` to fix Dependabot alerts (#230-241)
  * Bump protobufjs, @protobufjs/utf8, brace-expansion, and authlib to fix Dependabot alerts (#178, #242-250)
* **Repo:**
  * Migrate Yarn 1 (Classic) to Yarn 4 (Berry) via corepack across all six yarn projects; corepack-only delivery, `nodeLinker: node-modules`; CI/Docker/build scripts updated; clean lock + zero install warnings (#1008)
* **CI:**
  * Switch dev-worker runners to ephemeral mode (one job per container, unique name per run) with build caches persisted in a named Docker volume (cargo, yarn, go, playwright); bake Go 1.25 into the runner image and skip `setup-go` on dev-worker; drop the now-redundant `--clear-cache` / `--rotate-cache` / `--rotate-at` flags

## April 2026 - v0.24.0

* **Analytics:**
  * Add `parse_block` table UDF for generic block inspection with transit-to-JSONB conversion (#1001)
* **CLI:**
  * Add unified diff output to `micromegas-screens plan` and `apply` for updated screens (#998)
* **Bug Fixes:**
  * Fix byteLength crash on 0-row Arrow tables in notebook status text (#1000)
  * Fix notebook variable URL desync on rapid updates and datasource reverting to default on change (#1003)
  * Fix flamechart WASD zoom continuing after key release in Chrome (#1013)
* **Web App:**
  * Show flamechart span duration in nanoseconds when below 1 microsecond
* **Docs:**
  * Add `parse_block` to functions reference (#1002)
  * Blog post: From Observability to Candor (#996)
* **Repo:**
  * Normalize line endings to LF for existing files (#997)
* **Dependencies:**
  * Update DataFusion to 52.5 (#1009)
  * Update pyarrow to ^23.0.0 (#1006)
* **Security:**
  * Update `rand` to 0.9 to fix unsoundness in `rand::rng()` (Dependabot #201, #202)
  * Bump protobufjs, protocol-buffers-schema, authlib, and rustls-webpki to fix Dependabot alerts (#205-209)
  * Bump dompurify to 3.4.0 to fix Dependabot alerts (#203, #204)
  * Bump pytest, cryptography, and opentelemetry-go SDK to fix Dependabot alerts (#198, #199, #200)
  * Bump vite, lodash, and lodash-es to fix Dependabot alerts
  * Bump serialize-javascript, handlebars, cryptography, and Pygments to fix Dependabot alerts
  * Bump picomatch, brace-expansion, yaml, and requests to fix Dependabot alerts

## March 2026 - v0.23.0

* **CI:**
  * Add container-based self-hosted runner infrastructure for faster CI builds on developer workstations
  * Add check-runner workflow to dynamically route builds between dev-worker and GitHub-hosted runners
  * Add nightly cache rotation with `--rotate-at` flag for built-in scheduling
* **Docs:**
  * Document JSONPath filter predicate syntax (SQL/JSON path) in query guide (#979)
* **Analytics:**
  * Add CSV table provider and `StaticTablesConfigurator` for auto-discovery of CSV/JSON tables via `MICROMEGAS_STATIC_TABLES_URL` (#946)
  * Downgrade extensionless file warning to debug in `StaticTablesConfigurator` (#954)
  * Scope merge session context to insert time range to reduce memory during compaction (#963)
  * Add `FlightSqlServer` builder to eliminate boilerplate when assembling a FlightSQL server (#955)
  * Add `Send + Sync` bounds to `MergerMaker` type alias for async view factories (#972)
  * Add `LakehouseContext::from_env()` convenience constructor to deduplicate initialization (#969)
  * Allow `jsonb_each` to accept arbitrary expression arguments like `jsonb_parse(...)` (#978)
  * Add `jsonb_array_elements` UDTF to unnest JSONB arrays into rows (#977)
  * Add `jsonb_array_length` scalar UDF for counting JSONB array elements (#976)
  * Allow `expand_histogram` to accept expression arguments and Dictionary-wrapped scalars (#983)
* **Ingestion:**
  * Add `WebIngestionService::from_env()` convenience constructor to deduplicate initialization (#973)
* **Object Storage:**
  * Use `parse_url_opts` to honor environment variable credentials for S3/GCS/Azure (#948)
  * Fix env var credential parsing by lowercasing keys for `object_store` case sensitivity bug (#951)
* **CLI:**
  * Add `--file` option to `micromegas-query` for reading SQL from a file or stdin (#941)
* **Web App:**
  * Add `managed_by` column to screens table for source-control tracking
  * Show warning banner when editing a source-controlled screen
  * Add Bearer token authentication to analytics-web-srv (alongside cookie auth)
  * Add `initialFrom`/`initialTo` options to flamegraph cell for pre-zoomed initial view
  * Resolve `$cell.selected.column` macros in table column overrides (#975)
  * Format timestamps in cell selection display panel
  * Show actual cell names in override editor help text
  * Halt notebook execution when a cell is blocked on a missing selection
* **CLI:**
  * Add `micromegas-screens` tool for managing screens as code with Terraform-inspired workflow (init, import, pull, plan, apply, list)
  * Add HTTP client (`WebClient`) for analytics-web-srv REST API
* **Claude Code Plugin:**
  * Add shareable micromegas plugin with micromegas-query skill for querying observability data via SQL
  * Extract pr, design, design-review, and branch-review skills to standalone dev-skills plugin (#960)
* **Repo:**
  * Add `.gitattributes` for LF line endings, ignore `.claude/settings.local.json` (#962)
* **Dependencies:**
  * Update lz4_flex to 0.12.1 to fix memory information leak vulnerability
  * Fix 6 dependabot security alerts: flatted 3.4.2, rustls-webpki 0.103.10, grpc-go 1.79.3
  * Fix rustls-webpki alert in datafusion-wasm Cargo.lock
  * Update DataFusion to 52.4.0 (#964)

## March 2026 - v0.22.0

* **Tracing:**
  * Fix async span depth across yield points and spawn boundaries with `SpanContextFuture` (#917)
  * Fix async span depth inconsistency by capturing depth at future creation time instead of poll time (#927)
* **Telemetry:**
  * Add default system properties (exe, username, hostname, CPU, memory, OS) to process metadata (#380)
* **Database Migration (REQUIRED before upgrade):**
  * Run the following SQL on your database before deploying this version:
    ```sql
    CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS processes_process_id_unique ON processes(process_id);
    CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS streams_stream_id_unique ON streams(stream_id);
    CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS blocks_block_id_unique ON blocks(block_id);
    ```
  * If you have existing duplicate rows, clean them up first using `delete_duplicate_blocks()`, `delete_duplicate_streams()`, `delete_duplicate_processes()` or manual deduplication
  * Validate concurrent indexes before completing v2→v3 migration to prevent invalid indexes from being silently accepted (#911)
* **Dependencies:**
  * Fix CVE-2026-32141: upgrade flatted to 3.4.1 to fix unbounded recursion DoS
  * Update tower-http from 0.5 to 0.6
  * Update quinn-proto to 0.11.14 to fix unauthenticated DoS vulnerability
  * Update DataFusion to 52.3 (#907)
  * Update DataFusion to 52.2 and remove LimitPushdown workaround (#882)
* **Notebook Enhancements:**
  * Add flame graph cell type with Three.js WebGL rendering for Perfetto-style CPU trace visualization (#917)
  * Rename $begin/$end macros to $from/$to and fix expansion in column overrides (#914)
  * Add Insert cell above/below options to cell context menu
  * Add Download CSV menu option for notebook cells (#900)
  * Add row selection to table cells with `$cell.selected.column` macros for interactive drill-down (#915)
  * Add cell result row references in macros — `$cell[N].column` syntax for chaining queries (#898)
  * Add Alt+PageUp/PageDown keyboard navigation for notebook cells (#909)
  * Close notebook cell editor on ESC key (#919)
  * Fix available variables showing orphaned URL params and unscoped variables
  * Fix timestamp values rendering as raw integers in macro expansion (#908, #910)
* **Analytics Web App:**
  * Fix config diff modal not showing non-cell changes (e.g. refresh interval) for notebooks
  * Fix time range picker calendar dismissing on every interaction (#930)
  * Fix page-level and swimlane cell scrollbar issues with flex layout
  * Fix swimlane TimeAxis tick alignment to prevent horizontal overflow
  * Add Ctrl+S keyboard shortcut to save screens
  * Fix crash on first notebook cell execution when loading state has no data field
  * Add auto-refresh feature with configurable intervals and execution-aware spinner (#892)
  * Add SQL editor improvements: horizontal scrolling, format button, and Ctrl+Enter to run
  * Replace sql-formatter-plus with sql-formatter (zero-dependency, actively maintained)
  * Fix concurrent token refresh race in authenticatedFetch
  * Remove local_query screen type (#871)
* **Security:**
  * Fix minimatch ReDoS vulnerabilities across JS packages (Dependabot alerts #116–#123)
  * Bump serialize-javascript to 7.0.3 and OpenTelemetry SDK to 1.40.0 (Dependabot security alerts)
  * Bump authlib to 1.6.9, dompurify to 3.2.7+, and immutable to 5.1.5 (Dependabot security alerts)
  * Bump dompurify to 3.3.2, jest to 30, and @types/jest to 30 (Dependabot security alerts)
  * Migrate window.location.href assignments to navigateTo() wrapper for jsdom 26 compatibility
* **Analytics:**
  * Add `jsonb_path_query_first` and `jsonb_path_query` UDFs for JSONPath traversal of JSONB columns (#920)
  * Extend `jsonb_each` to support array inputs (#920)
  * Bump jsonb dependency from 0.5.3 to 0.5.5 (#920)
  * Add `process_spans(process_id, types)` table function for cross-thread and async span analysis (#917)
  * Add `hash` column to `async_events` view for scope identification (#917)
  * Extract `get_process_thread_list` as shared utility for process stream discovery
* **Query CLI:**
  * Require `--begin` flag and default `--end` to now to prevent accidental full-range queries
  * Add `--all` flag to explicitly query the entire time range without time filtering
* **Documentation:**
  * Migrate all documentation URLs from madesroches.github.io to micromegas.info
  * Remove dead troubleshooting link from Grafana plugin README
* **Build:**
  * Add wasm-opt optimization step to WASM Docker builder for smaller binaries
  * Fix wasm-opt corrupting externref table by upgrading binaryen and enabling reference-types
  * Add SHA256 checksum verification for binaryen download in WASM builder

## February 2026 - v0.21.0
* **Security:**
  * Bump rollup to 4.59.0 to fix CVE (arbitrary file write) across all JS packages
* **Build:**
  * Gate server-only dependencies behind `server` feature flag on `micromegas-telemetry` and `micromegas` crates (#855)
* **DataFusion Extensions:**
  * Add `jsonb_each` table function to expand JSONB objects into key-value rows (#860)
* **Notebook Enhancements:**
  * Switch cell selection from single-click to double-click and add Edit cell to context menus
  * Move SERIES_COLORS to shared chart-constants module
  * Fix health check URL in start_analytics_web.py
  * Add column override support to transposed table cells (#868)
  * Add process info notebook with analysis links and clamped time range
  * Fix unsaved edits detection by snapshotting baseline JSON on edit start
  * Add copy and edit buttons to notebook view source panel
  * Add row hiding to transposed table cell via right-click context menu
  * Sort cell types alphabetically in Add Cell dialog
  * Stabilize hook references to prevent unnecessary re-renders in notebook components
* **Documentation:**
  * Add screenshots to notebook documentation pages
  * Fix rustdoc warnings in analytics crate
  * Add Interactive Notebooks for Observability presentation with Reveal.js + Micromegas brand theme
  * Add notebook documentation: cell types reference, variables, execution model, and web app overview
  * Reorganize mkdocs nav into Analytics Web App, Integrations, and Operations sections
  * Add notebook datasource option to dropdown variable cells (#861)
  * Add transposed table cell type for key-value layout of SQL results
  * Replace single-letter cell type icons with Lucide components
  * Stop markdown link clicks from opening cell editor in notebook tables
* **Analytics Web App:**
  * Add LZ4 compression to Arrow IPC streams for dramatically smaller network transfers
  * Raise FlightSQL client max decoding message size to 100MB
* **Python CLI:**
  * Add `micromegas-query` and `micromegas-logout` as installed CLI entry points via `pip install micromegas`
  * Move CLI module into package directory for proper entry point resolution
  * Remove legacy single-purpose CLI scripts superseded by general-purpose `micromegas-query`
* **Analytics Web App:**
  * Adapt chart Y-axis scale to visible series when hiding in multi-series charts (#836)
  * Hide Y-axis when all series for a unit are hidden
* **WASM UDF Extensions:**
  * Extract JSONB and histogram UDFs into shared `micromegas-datafusion-extensions` crate
  * Register extension UDFs in WASM query engine for browser-side SQL parity with server
  * Add WASM integration tests for JSONB and histogram UDFs
* **Tracing:**
  * Fix empty-string backward compatibility in opt_uuid_from_string for 0.20.0 clients (#850)
* **Security:**
  * Upgrade ajv from 6.12.6 to 6.14.0 to fix CVE-2025-69873 (dependabot #109)
  * Fix minimatch ReDoS vulnerability via resolution override to v10.x (dependabot alerts #104, #105)
  * Migrate Grafana eslint config to native flat config, remove @eslint/eslintrc dependency
* **Notebook Cross-Cell Queries:**
  * Add notebook-local query support via WASM DataFusion engine (#815)
  * Cells with `dataSource: 'notebook'` execute SQL in-browser against other cells' results
  * Remote cell results automatically registered in WASM context for cross-cell references
  * Add `execute_and_register` and `deregister_table` methods to WASM engine
  * Add "Notebook (local)" option to data source dropdown in cell editor
  * Live download progress (rows/bytes) and execution time in cell title bars
  * Extract shared `serialize_to_ipc` helper in datafusion-wasm crate
* **WASM Tracing (#817):**
  * Implement WASM support for micromegas-tracing and telemetry-sink
  * Add tracing to WASM query engine and fix wasm dispatch init
  * Add Send + Sync bounds to EventSink trait
  * Set default-features = false for micromegas-tracing in workspace
  * Remove unused feature flags from tracing and telemetry-sink
  * Enable SHOW TABLES in WASM query engine
* **Horizontal Group Cell:**
  * Implement horizontal group (hg) cell type for side-by-side cell layout in notebooks (#821)
  * Pass variable value/onValueChange to hg children, render combobox inline
  * Add DataSourceField to hg child editors
  * Show running status and spinner on hg cells
  * Add Run button for HG child cells in editor panel
  * Fix drag-into-group oscillation, drag-out, and reorder preview
  * Fix HG editor rename, child fetch progress, and child variable auto-run
  * Fix HG child content click to open child editor directly
  * Clean up hg child state on removal and migrate on rename
  * Add tests for HorizontalGroupCell (36 tests)
* **Multi-Query Chart Cells (#749):**
  * Implement multi-query chart cells with per-query data sources
  * Refactor CellState.data from Table | null to Table[] for multi-result support
  * Fix multi-series chart Y-axis unit auto-scaling
  * Fix chart cells defaulting to notebook instead of global data source
  * Fix single-query chart not using configured unit and label
  * Fix tooltip XSS and deduplicate SERIES_COLORS
  * Portal chart tooltips to document.body to prevent overflow clipping
  * Stop chart header clicks from selecting cell, fix clipped tooltips
* **Compact Notebook UI:**
  * Implement compact borderless notebook UI with minimal visual chrome
  * Add fade-on-idle behavior for cell metadata with three-state fade machine
  * Add always-visible loading spinner to cell status area
  * Unify status text placement between groups and cells
  * Restyle pagination bar to centered minimal design
  * Restyle notebook tables to minimal lines with prominent header
  * Restyle notebook log cells to match compact table design
  * Fix pagination overlap, hardcoded dark colors, and hidden HG status
  * Fix selection indicator layout shift with always-present border
  * Fix fade-on-idle reveal for fast cells and add hover delay
* **Notebook Enhancements:**
  * Add notebook pagination and per-cell auto-run (#823)
  * Add reference table cell type for inline CSV data (#824, #827)
  * Add duplicate cell action to notebook (#834)
  * Resolve relative time ranges at cell execution time in all renderers
  * Fix refresh not updating relative time spans
  * Fix data source race condition overwriting config on load
  * Stop column header sort click from opening cell editor (#829)
  * Deprecate non-notebook screen types for creation
* **Log Cell Improvements:**
  * Make log cell display resilient to unexpected or missing columns (#826)
  * Extract renderLogColumn to shared log-utils module
  * Preserve SQL column order in log renderers instead of reordering known columns
  * Hide data source selector for reference table cells
* **Code Quality:**
  * Extract 6 hooks from NotebookRenderer (1154 → 736 lines)
  * Extract buildCellRendererProps to unify cell rendering prop assembly
  * Consolidate AddCellModal and AddChildModal into shared component
  * Deduplicate datasourceVariables computation in NotebookRenderer
* **Security:**
  * Fix CVE-2025-69873: upgrade ajv 8.17.1 to 8.18.0
  * Bump qs to 6.14.2 to fix CVE-2026-2391
* **CI/Build:**
  * Run native and WASM CI checks in parallel
  * Fix wasm-builder to copy full Rust workspace for path dependencies
  * Add build-skip workflow for required check satisfaction
  * Install clang in wasm-builder Docker image for native dependency compilation
* **Documentation:**
  * Fix homepage and documentation URLs in Cargo.toml
  * Update README roadmap with v0.20.0 and current notebook focus
* **Scripts:**
  * Add --release flag to start_services.py and run binaries directly

## February 2026 - v0.20.0
* **Client-Side WASM Query Execution:**
  * Add `local_query` screen type running DataFusion SQL in the browser via WebAssembly (#806, #807, #808, #810)
  * Progressive row count and byte size display during source query fetch
  * Auto-run checkbox for local query execution on text changes
  * Rename datafusion-wasm to micromegas-datafusion-wasm with CI integration
  * Shared WASM builder Dockerfile stage for Docker builds
* **Configurable Data Sources:**
  * Add configurable data sources for analytics web app (#793)
  * Per-screen and per-cell data source selection (#794)
  * Datasource variable type for notebook data source selection (#800)
  * Data source selector on Processes, ProcessMetrics, and ProcessLog pages
  * Protected default data source from deletion and flag removal
* **Notebook Enhancements:**
  * Add Perfetto export cell type for notebooks (#771)
  * Add expression variable type for adaptive time_bin_duration (#782)
  * Add swimlane notebook cell type for visualizing concurrent events (#769)
  * Add drag-to-zoom time range selection to notebook charts (#768)
  * Add property timeline notebook cell type (#766, #762)
  * Re-execute notebook cells when time range changes (#768)
  * Add query guide links to SQL editor cells (#751)
  * Move variable cell input to title bar to reduce vertical space (#779)
  * Move save buttons to title bar, add config diff modal (#780)
  * Extract useExposeSaveRef hook, remove duplicate SaveFooter from renderers (#780)
  * Add zoom in/out buttons to time range control (#804)
* **Query & Data Features:**
  * Add multi-column query variables with $variable.column syntax (#753)
  * Add table URL support with column overrides (#750)
  * Add unit formatting system for charts (#755)
  * Allow hiding columns via right-click context menu (#790)
* **Client-Side Perfetto Trace Generation:**
  * Replace generate_trace endpoint with client-side trace fetching (#784)
  * Add gzip compression to analytics-web-srv endpoints (#784)
  * Add abort signal support for trace downloads
* **Performance Optimizations:**
  * Add parquet file content cache to reduce object storage reads (#757, #758)
  * Parallelize JIT for Perfetto trace thread span generation (#759, #772)
  * Implement pipelined query planning for Perfetto trace generation (#759)
* **Unreal Engine:**
  * Support 32-bit and 64-bit metrics (#786)
* **Dependencies:**
  * Update DataFusion to 52.1 and Arrow/Parquet to 57.2 (#756), Arrow to 57.3
* **Security:**
  * Update bytes crate to 1.11.1 to fix CVE-2026-25541 (#767)
  * Upgrade jsonwebtoken to 10.3 to fix type confusion vulnerability (#760)
  * Fix dependabot security alerts: protobuf and time (#787)
  * Bump cryptography from 46.0.3 to 46.0.5 (#801)
* **Analytics Web App:**
  * Add welcome landing page for madesroches.github.io/micromegas (#785)
  * Hide admin icon in sidebar for non-admin users (#802)
  * Add Process Details link to PivotButton navigation (#777)
  * Remove process list from available screen types (#791)
  * Fix Perfetto trace generation missing data source parameter (#805)
* **Documentation:**
  * Document delete_duplicate SQL functions and reorganize admin docs (#752)
  * Link documentation site in crate READMEs and PyPI metadata (#798)
  * Add GoatCounter analytics to all public pages (#796)
* **Code Quality:**
  * Remove old perf_report task folder
  * Remove column name transformation in process list tables (#744)
  * Refactor analytics-web-srv main.rs into focused functions
  * Delete orphaned queries.rs

## January 2026 - v0.19.0
* **User-Defined Screens:**
  * Add user-defined screens feature (#707)
  * Add table screen type with generic SQL viewer (#726)
  * Add notebook screen type with multi-cell layout (#728)
  * Refactor notebook cells to follow Open-Closed Principle (#729)
  * Notebook OCP refactoring and URL variable synchronization (#730)
  * Add syntax highlighting to notebook cell editors (#731)
  * Delta-based URL handling for notebook variables and time range (#734)
  * Add copy/paste support for time ranges (#735)
  * Decouple URL param ownership from ScreenPage to renderers (#736)
  * Add admin section with export/import screens (#737)
* **Data Integrity:**
  * Add delete_duplicate_streams and delete_duplicate_processes UDFs (#721)
* **Analytics & Query Features:**
  * Add expand_histogram table function and bar chart toggle (#720)
  * Unify chart and property timeline queries (#732)
  * Enable dictionary encoding preservation for web app (#727)
* **Analytics Web App:**
  * MVC view state refactor and XYChart generalization (#718)
  * Migrate remaining pages to useScreenConfig and remove useTimeRange (#719)
  * Add dynamic page titles (#712)
  * Consolidate API endpoints under /api prefix (#711)
  * Disable source maps in production builds (#710)
  * Fix blank page on hard refresh for deep URLs (#713)
* **Infrastructure:**
  * Add micromegas_app database creation to service startup (#705)
* **Security:**
  * Fix lodash prototype pollution vulnerability (CVE-2025-13465) (#725)
  * Fix Dependabot alert #91: upgrade diff to 8.0.3 (#708)
  * Fix dependabot alerts for grafana plugin dependencies (#704)
  * Fix 4 dependabot security alerts (#703)
* **Documentation:**
  * Add plans for unified metrics query and dictionary preservation (#724)
  * Add notebook screen design and generalized metrics chart plan (#716)
  * Update changelog and readme with unreleased changes (#722)
  * Update unified observability presentation slides (#706)
  * Add unified observability presentation link (#702)

## January 2026 - v0.18.0
* **Reliability & Data Integrity:**
  * Add periodic duplicate block cleanup to maintenance daemon (#700)
  * Prevent duplicate insertion for blocks, streams, and processes (#691)
  * Add delete_duplicate_blocks UDF (#689)
  * Fix queue_size going negative on timeout in http_event_sink (#699)
* **Ingestion & Client:**
  * Add proper HTTP error codes and client retry logic (#696)
* **Analytics & Query Features:**
  * Implement Arrow IPC streaming for query API (#685)
  * Enable SHOW TABLES and information_schema support (#687)
  * Add global LRU metadata cache for partition metadata (#674)
  * Add jsonb_object_keys UDF (#673)
  * Add property timeline feature for metrics visualization (#684)
* **Tracing & Instrumentation:**
  * Improve #[span_fn] rustdoc documentation (#676)
  * Fix async span parenting and add spawn_with_context helper (#675)
  * Add thread block parsing trace and tooling config (#686)
* **Analytics Web App:**
  * Migrate from Next.js to Vite for dynamic base path support (#667)
  * Pivot split button for process view navigation (#682)
  * Metrics chart scaling and time units improvements (#681)
  * Auto-refresh auth token on 401 API responses (#680)
  * Improve process info navigation and cleanup trace screen (#669)
  * Fix custom queries being reset when filters change (#670)
* **Python CLI:**
  * HTTPS URI support and executable scripts (#683)
* **Unreal Engine:**
  * Add more metrics and process info to telemetry plugin (#672)
* **Security:**
  * Fix urllib3 decompression bomb vulnerability (CVE-2026-21441) (#695)
  * Fix security vulnerabilities in qs and rsa dependencies (#693)
  * Fix esbuild security vulnerability (GHSA-67mh-4wv8-2f99) (#671)

## December 2025 - v0.17.0
 * **Analytics Web App Major Rework:**
   * Complete UI redesign with dark theme and Micromegas branding (#621, #622, #623)
   * Add Grafana-style time range picker with relative and absolute time support (#631)
   * Add performance analysis screen with thread coverage timeline (#642, #643)
   * Add Perfetto trace integration with split button for browser/download (#660, #661)
   * Add process metrics screen with time-series charting (#639)
   * Add process properties display panel (#634)
   * Add multi-word search to process list and log screens (#632, #633)
   * Allow custom limit values in process log view (#627, #628)
   * Improve time column formatting in process logs (#624)
   * Pass time range through process navigation links (#636)
   * Add schema documentation links to SQL panels (#635)
   * UX improvements and polish (#645, #647)
 * **Deployment & Configuration:**
   * Add per-service Docker images and modernize build scripts (#637, #649)
   * Add BASE_PATH and MICROMEGAS_PORT env vars for reverse proxy deployments (#650, #651, #654, #656, #658, #659)
 * **Unreal Engine:**
   * Add scalability and VSync context to telemetry (#625)
   * Document API key authentication (#629)
 * **Security & Bug Fixes:**
   * Fix CVE-2025-66478: Update Next.js to 15.5.7 (#626)
   * Fix urllib3 security vulnerabilities and OIDC token validation bug (#641)
   * Fix UTF-8 user attribution headers with percent-encoding (#638)
   * Handle empty MICROMEGAS_TELEMETRY_URL environment variable (#644)
 * **Documentation:**
   * Fix documentation dark mode readability (#648)
 * **Code Quality:**
   * Fix rustdoc bare URL warnings in auth crate (#630)

## November 2025 - v0.16.0
 * Released [version 0.16.0](https://crates.io/crates/micromegas)
 * **New: HTTP Gateway:**
   * Add HTTP Gateway with Authentication and Security Features (#597)
 * **Analytics Web App:**
   * Add OIDC authentication to analytics web app (#596)
 * **Authentication:**
   * Fix ID token expiration and add multi-provider OIDC support (#608)
   * Fix OIDC authentication and token refresh issues (#590)
 * **Analytics & Query Features:**
   * Optimize JSONB UDFs for dictionary-encoded column support (#593)
   * Fix timestamp binding in retire_partition_by_metadata UDF (#606)
   * Handle empty incompatible partitions and fix thrift buffer sizing (#602)
 * **Grafana Plugin:**
   * Fix Grafana plugin packaging and document release process (#601)
   * Fix secureJsonData undefined error and rename plugin to Micromegas FlightSQL (#603)
 * **Security & Dependencies:**
   * Fix js-yaml prototype pollution vulnerability (CVE-2025-64718) (#592)
   * Upgrade DataFusion from version 50.2.0 to 51.0.0 (#598)
   * Fix LIMIT pushdown in all TableProvider implementations (#600)
 * **Documentation:**
   * Document auth_provider parameter and deprecate headers in Python API (#595)
 * **Build & CI:**
   * Enable Claude to submit PR reviews and issue comments (#605)
   * Claude PR Assistant workflow (#604)

## November 2025 - v0.15.0
 * Released [version 0.15.0](https://crates.io/crates/micromegas)
 * **New: Authentication Framework (micromegas-auth crate):**
   * Add authentication framework with OIDC and API key support (#546)
   * Implement OIDC authentication for Rust services and Python client (#548)
   * Add OIDC authentication support to CLI tools (#549)
   * Add OAuth 2.0 client credentials support for service accounts (#552)
   * Add HTTP authentication to ingestion service (#551)
   * Unified JWKS architecture for service accounts (#547)
   * Refactor OIDC connection to library module (#588)
 * **Grafana Plugin (v0.15.0 - First release from main repo):**
   * Integrate Grafana FlightSQL datasource plugin into main repository (#554)
   * Implement OAuth 2.0 authentication for Grafana plugin (#564)
   * Add variable query editor and datasource migration tools (#585)
   * Rename Grafana plugin to follow official naming guidelines (#583)
   * Implement CI/CD pipeline for Grafana plugin (#558)
   * Update Grafana plugin SDK to 11.6.7 and fix security vulnerabilities (#555)
   * Fix 28 Dependabot security vulnerabilities (#556)
 * **Authentication & Security:**
   * Rework AuthProvider to use request validation (#571)
   * Refactor MultiAuthProvider for extensibility (#569)
   * Add client IP logging to server observability (#566)
   * Comprehensive authentication documentation in admin guide (#550)
 * **Unreal Engine:**
   * Modernize Unreal telemetry sink module (#584)
 * **Server Enhancements:**
   * Add gRPC health check endpoint (#570)
 * **Build & CI:**
   * Fix CI linker crashes and improve build reliability (#572)
   * Fix documentation build by installing mold linker (#573)
 * **Documentation:**
   * Add build tools installation before build steps (#582)
   * Update build prerequisites (#581)
   * Add documentation links to all Rust crate READMEs (#578)
   * Update high-frequency observability presentation (#574)
   * Clean up presentation files and update docs to use yarn (#568)
   * Clean up task documentation and improve authentication docs (#567)
   * Consolidate and streamline Grafana and monorepo documentation (#559)
   * Update documentation links to use hosted docs and fix markdown formatting (#563)
   * Remove Grafana section from README (#560)
 * **Planning:**
   * Add plan for query variable time filter feature (#580)
   * Grafana plugin repository merge planning and Phase 1.1 completion (#553)

## October 2025 - v0.14.0
 * Released [version 0.14.0](https://crates.io/crates/micromegas)
 * **Performance & Storage Optimizations:**
   * Complete properties to dictionary-encoded JSONB migration (#521)
   * Properties writing optimization with ProcessMetadata and BinaryColumnAccessor (#522, #524)
 * **Analytics & Query Features:**
   * Add Dictionary<Int32, Binary> support to jsonb_format_json UDF (#536)
   * Add SessionConfigurator for custom table registration (#531)
   * Add file existence validation to json_table_provider (#532)
   * Enable property_get UDF to access JSONB columns (#520)
   * Add support for empty lakehouse partitions (#537)
 * **Bug Fixes & Reliability:**
   * Fix NULL value handling in SQL-Arrow bridge with integration tests (#541)
   * Fix null decoding error in list_partitions table function (#540)
   * Fix null decoding error for file_path in retire_partitions (#539)
 * **Documentation & Presentations:**
   * Add High-Frequency Observability presentation (OSACON 2025) (#527, #528, #529, #533)
   * Update presentation template to new Vite-based build (#525)
 * **Security & Dependencies:**
   * Update Vite to 7.1.11 to fix security vulnerabilities (#526, #542)
   * Update DataFusion and Arrow Flight dependencies (#519)
   * cargo update (#530)
 * **Code Quality:**
   * Fix rustdoc HTML tag warnings in analytics crate (#534)
 * **Future Work:**
   * Analytics Server Authentication Plan (#543)

## September 2025 - v0.13.0
 * Released [version 0.13.0](https://crates.io/crates/micromegas)
 * **Performance & Storage Optimizations:**
   * Dictionary encoding for properties columns with comprehensive UDF support (#506, #507, #508, #510, #511)
   * Properties to JSONB UDF for efficient storage and querying (#515)
   * Arrow string column accessor with full dictionary encoding support (#511)
   * Production performance analysis of dictionary encoding effectiveness (#508)
   * Fixed parquet metadata race conditions with separation strategy (#502, #504)
   * Optimized lakehouse partition queries by removing unnecessary file_metadata fetches (#499)
   * Scalability improvements for high-volume environments (#497, #498)
 * **Schema Evolution & Admin Features:**
   * Incompatible partition retirement feature for schema evolution (#512)
   * Enhanced error logging in CompletionTrackedStream (#503)
   * Improved PostgreSQL container management in development environment (#500)
 * **Monitoring & Analytics:**
   * Added `log_stats` SQL aggregation view for log analysis by severity and service (#495, #505)
   * Enhanced documentation with log_stats view in schema reference
 * **Code Quality & Development:**
   * Organized project documentation and completed task archival (#513, #514, #517)
   * Dictionary encoding analysis archived due to Parquet limitations (#516)

## September 2025
 * Released [version 0.12.0](https://crates.io/crates/micromegas)
 * **Major Features:**
   * Comprehensive async span tracing with `micromegas_main` proc macro (#451)
   * Named async span event tracking with improved API ergonomics (#475)
   * Async span depth tracking for performance analysis (#474)
   * Async trait tracing support in `span_fn` macro (#469)
   * Perfetto async spans support with trace generation (#485)
   * HTTP gateway for easier interoperability (#433, #435, #436)
   * JSONB support for flexible data structures (#409)
 * **Infrastructure & Performance:**
   * Consolidate Perfetto trace generation to use SQL-powered implementation (#489)
   * Query latency tracking and async span instrumentation optimization (#468)
   * Replace custom interning logic with `internment` crate (#430)
   * Optimize view_instance metadata requests (#450)
   * Convert all unit tests to in-memory recording (#472)
 * **Documentation & Developer Experience:**
   * Complete Python API documentation with comprehensive docstrings (#491)
   * Complete SQL functions documentation with all missing UDFs/UDAFs/UDTFs (#470)
   * Visual architecture diagrams in documentation (#462)
   * Unreal instrumentation documentation (#492)
   * Automated documentation publishing workflow (#444)
 * **Security & Dependencies:**
   * Fix CVE-2025-58160: Update tracing-subscriber to 0.3.20 (#490)
   * Update DataFusion, tokio and other dependencies (#429, #476)
   * Rust edition 2024 upgrade with unsafe operations fixes (#408)
 * **Web UI & Export:**
   * Export Perfetto traces from web UI (#482)
   * Analytics web app build fixes and documentation updates (#483)
 * **Cloud & Deployment:**
   * Docker deployment scripts (#422)
   * Amazon Linux setup script (#423)
   * Cloud environment configuration support (#426)
   * Configurable PostgreSQL port via MICROMEGAS_DB_PORT (#425)

## July 2025
 * Released [version 0.11.0](https://crates.io/crates/micromegas)
 * Working on http gateway for easier interoperability
 * Add export mechanism to view materialization to send data out as it is ingested

## June 2025
 * Released [version 0.10.0](https://crates.io/crates/micromegas)
 * Process properties in measures and log_entries
 * Better histogram support
 * Processes and streams views now contain all processes/streams updated in the requested time range - based on SqlBatchView.

## May 2025
 * Released [version 0.8.0](https://crates.io/crates/micromegas) and [version 0.9.0](https://crates.io/crates/micromegas)
 * Frame budget reporting
 * Histogram support with quantile estimation
 * Run seconds & minutes tasks in parallel in daemon
 * GetPayload user defined function
 * Add bulk ingestion API for replication

## April 2025
 * Released [version 0.7.0](https://crates.io/crates/micromegas)
 * Perfetto trace server
 * DataFusion memory budget
 * Memory optimizations
 * Fixed interning of property sets
 * More flexible trace macros

## March 2025
 * Released [version 0.5.0](https://crates.io/crates/micromegas)
 * Better perfetto support
 * New rust FlightSQL client
 * Unreal crash reporting

## February 2025
 * Released [version 0.4.0](https://crates.io/crates/micromegas)
 * Incremental data reduction using sql-defined views
 * System monitor thread
 * Added support for ARM (& macos)
 * Deleted analytics-srv and the custom http python client to connect to it
 
## January 2025
 * Released [version 0.3.0](https://crates.io/crates/micromegas)
 * New FlightSQL python API
   * Ready to replace analytics-srv with flight-sql-srv

## December 2024
 * [Grafana plugin](https://github.com/madesroches/micromegas/tree/main/grafana)
 * Released [version 0.2.3](https://crates.io/crates/micromegas)
 * Properties on measures & log entries available in SQL queries

## November 2024
Released [version 0.2.1](https://crates.io/crates/micromegas)

 * FlightSQL support
 * Measures and log entries can now be tagged with properties
   * Not yet available in SQL queries

## October 2024
Released [version 0.2.0](https://crates.io/crates/micromegas)

 * Unified the query interface
   * Using `view_instance` table function to materialize just-in-time process-specific views from within SQL
 * Updated python doc to reflect the new API: https://pypi.org/project/micromegas/

## September 2024
Released [version 0.1.9](https://crates.io/crates/micromegas)

 * Updating global views every second
 * Caching metadata (processes, streams & blocks) in the lakehouse & allow sql queries on them

## August 2024
Released [version 0.1.7](https://crates.io/crates/micromegas)

 * New global materialized views for logs & metrics of all processes
 * New daemon service to keep the views updated as data is ingested
 * New analytics API based on SQL powered by Apache DataFusion

## July 2024
Released [version 0.1.5](https://crates.io/crates/micromegas)

Unreal
 * Better reliability, retrying failed http requests
 * Spike detection

Maintenance
 * Delete old blocks, streams & processes using cron task

## June 2024
Released [version 0.1.4](https://crates.io/crates/micromegas)

Good enough for dogfooding :)

Unreal
 * Metrics publisher
 * FName scopes

Analytics
 * Metric queries
 * Convert cpu traces in perfetto format

## May 2024
Released [version 0.1.3](https://crates.io/crates/micromegas)

Better unreal engine instrumentation
  * new protocol
  * http request callbacks no longer binded to the main thread
  * custom authentication of requests

Analytics
  * query process metadata
  * query spans of a thread

## April 2024
Telemetry ingestion from rust & unreal are working :) 

Released [version 0.1.1](https://crates.io/crates/micromegas)

Not actually useful yet, I need to bring back the analytics service to a working state.

## January 2024
Starting anew. I'm extracting the tracing/telemetry/analytics code from https://github.com/legion-labs/legion to jumpstart the new project. If you are interested in collaborating, please reach out.
