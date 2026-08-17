use crate::data_lake_config::DataLakeConfig;
use crate::data_lake_connection::{DataLakeConnection, connect_to_data_lake};
use crate::remote_data_lake::migrate_db;
use crate::write_audience::WriteAudience;
use anyhow::Context;
use bytes::Buf;
use micromegas_telemetry::blob_storage::PutIfAbsent;
use micromegas_telemetry::block_wire_format;
use micromegas_telemetry::property::Property;
use micromegas_telemetry::property::make_properties;
use micromegas_telemetry::property::{PROPERTY_AUDIENCE, RESERVED_PROPERTY_PREFIX};
use micromegas_telemetry::stream_info::StreamInfo;
use micromegas_telemetry::wire_format::encode_cbor;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;
use thiserror::Error;
use uuid::Uuid;

static EMPTY_TRANSIT_METADATA_CBOR_BYTES: LazyLock<Vec<u8>> = LazyLock::new(|| {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(&Vec::<()>::new(), &mut buf)
        .expect("encoding an empty Vec to CBOR is infallible");
    buf
});

/// Sentinel for `dependencies_metadata` / `objects_metadata` on streams that
/// don't use the transit/POD wire format (e.g. OTLP). Existing readers decode
/// these BYTEA columns as `Vec<UserDefinedType>` and iterate; an empty Vec
/// makes those loops no-ops without touching consumer code.
pub fn empty_transit_metadata_cbor() -> &'static [u8] {
    &EMPTY_TRANSIT_METADATA_CBOR_BYTES
}

/// Format string for native streams (transit-encoded payload, CBOR envelope).
pub const FORMAT_TRANSIT: &str = "micromegas-transit";

/// Stream `format` value for OTel logs (one `ResourceLogs` proto per block payload).
pub const FORMAT_OTLP_LOGS: &str = "otlp/v1/logs";

/// Stream `format` value for OTel metrics (one `ResourceMetrics` proto per block payload).
pub const FORMAT_OTLP_METRICS: &str = "otlp/v1/metrics";

/// Stream `format` value for OTel traces (one `ResourceSpans` proto per block payload).
pub const FORMAT_OTLP_TRACES: &str = "otlp/v1/traces";

/// Error type for ingestion service operations.
/// Categorizes errors to enable proper HTTP status code mapping.
#[derive(Error, Debug)]
pub enum IngestionServiceError {
    /// Client-side errors (malformed input) - maps to 400 Bad Request
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Database errors - maps to 500 Internal Server Error
    #[error("Database error: {0}")]
    DatabaseError(String),

    /// Object storage errors - maps to 500 Internal Server Error
    #[error("Storage error: {0}")]
    StorageError(String),

    /// A re-registration of an existing `process_id` under a different audience than the one
    /// it was originally stamped with (AbAC Stage 5, #1373, §6). Maps to 403 Forbidden -- the
    /// one invariant that makes Stage 2's `MAX(audience)` per-process resolution
    /// (`ownership_rewrite.rs`) sound rather than merely assumed.
    #[error(
        "Audience conflict: process_id {process_id} was registered under audience {existing:?}, \
         this request carries {incoming:?}"
    )]
    AudienceConflict {
        process_id: Uuid,
        existing: String,
        incoming: String,
    },
}

/// Drops every client-supplied property whose key starts with the reserved `micromegas.`
/// namespace ([`RESERVED_PROPERTY_PREFIX`]), so that namespace can never be asserted from the
/// payload (AbAC Stage 5, #1373, §3). A dropped key is `warn!`-logged once per call, naming the
/// key -- a native client setting e.g. `micromegas.audience` was either doing the pre-Stage-5
/// self-stamp thing or probing, and both are worth seeing. Stripping rather than rejecting the
/// request with 400 keeps a legacy self-stamping producer's telemetry flowing on upgrade, even
/// though its self-stamp no longer takes effect.
///
/// `pub`, not private: `tests/write_audience_tests.rs` asserts this directly (see
/// `handler::build_webhook_request`'s doc comment for the identical precedent in
/// `micromegas-otel-ingestion`).
pub fn strip_reserved_properties(properties: Vec<Property>) -> Vec<Property> {
    properties
        .into_iter()
        .filter(|p| {
            if p.key_str().starts_with(RESERVED_PROPERTY_PREFIX) {
                warn!(
                    "dropping client-supplied reserved property {:?} -- the {RESERVED_PROPERTY_PREFIX} \
                     namespace is server-written only",
                    p.key_str()
                );
                false
            } else {
                true
            }
        })
        .collect()
}

/// Drops every client-supplied reserved-namespace property (see [`strip_reserved_properties`]),
/// then appends the server-written [`PROPERTY_AUDIENCE`] property when `audience` carries one.
/// Client input can therefore neither assert nor suppress the stamp. `audience: &WriteAudience`
/// of `None` writes no property at all -- absent, not empty, matching what
/// `OwnershipRewriteConfig.unstamped_audience` coalesces.
///
/// `pub`, not private -- see [`strip_reserved_properties`]'s doc comment for why.
pub fn finalize_process_properties(
    client: Vec<Property>,
    audience: &WriteAudience,
) -> Vec<Property> {
    let mut properties = strip_reserved_properties(client);
    if let Some(aud) = audience.as_str() {
        properties.push(Property::new(
            Arc::new(PROPERTY_AUDIENCE.to_string()),
            Arc::new(aud.to_string()),
        ));
    }
    properties
}

#[derive(Clone)]
pub struct WebIngestionService {
    lake: DataLakeConnection,
    ready_ok_until: Arc<Mutex<Option<Instant>>>,
}

impl WebIngestionService {
    pub fn new(lake: DataLakeConnection) -> Self {
        Self {
            lake,
            ready_ok_until: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn check_ready(&self) -> bool {
        let now = Instant::now();
        {
            let guard = self.ready_ok_until.lock().expect("readiness cache lock");
            if let Some(ok_until) = *guard
                && ok_until > now
            {
                return true;
            }
        }

        let probe_db = instrument_named!(
            sqlx::query("SELECT 1").execute(&self.lake.db_pool),
            "sql_readiness_probe"
        );
        let probe_blob = self.lake.blob_storage.probe();

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            tokio::join!(probe_db, probe_blob)
        })
        .await;

        match result {
            Ok((Ok(_), Ok(()))) => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = Some(Instant::now() + std::time::Duration::from_secs(1));
                true
            }
            _ => {
                let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
                *guard = None;
                false
            }
        }
    }

    /// Pre-seeds the readiness cache to `until`. Intended for testing only.
    #[doc(hidden)]
    pub fn set_ready_until(&self, until: Instant) {
        let mut guard = self.ready_ok_until.lock().expect("readiness cache lock");
        *guard = Some(until);
    }

    /// Reads MICROMEGAS_SQL_CONNECTION_STRING and MICROMEGAS_OBJECT_STORE_URI,
    /// connects to the data lake, runs ingestion migrations, and returns
    /// a ready-to-use service.
    pub async fn from_env() -> anyhow::Result<Arc<Self>> {
        let cfg = DataLakeConfig::from_env()?;
        let lake = connect_to_data_lake(&cfg.sql_connection_string, &cfg.object_store_uri).await?;
        migrate_db(lake.db_pool.clone())
            .await
            .with_context(|| "migrate_db")?;
        Ok(Arc::new(Self::new(lake)))
    }

    #[span_fn]
    pub async fn insert_block(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let block: block_wire_format::Block = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing block: {e}")))?;
        self.insert_block_typed(block).await
    }

    /// Inserts a block whose payload is already typed (no envelope round-trip on the caller side).
    ///
    /// The caller hands us a fully-built `Block`; we CBOR-encode the payload envelope once,
    /// write it to object storage, and INSERT the row. Used by the OTLP adapter where
    /// constructing the CBOR `Block` envelope just so `insert_block` could decode it
    /// would be wasted work.
    ///
    /// **Known gap, not yet closed (AbAC Stage 5b, follow-up to #1373, §7).** This method
    /// accepts any `process_id`/`stream_id` unconditionally -- there is no check that the
    /// authenticated caller is authorized to write to the process the block's `process_id`
    /// belongs to. A credential bound to audience A that knows a `process_id`/`stream_id`
    /// belonging to audience B can append events to B's process, and those events inherit B's
    /// stamped audience (§3) -- so B's readers see data B did not produce. This grants no read
    /// power (reading B still requires a read grant on B), but it is a real, tracked integrity
    /// gap: the fix is a write-side authorization gate (resolve the target's owning audience and
    /// let the auth layer decide) deliberately deferred to its own issue rather than folded into
    /// this stage -- see `tasks/1373_ingestion_stamping_plan.md` §7 for why.
    #[span_fn]
    pub async fn insert_block_typed(
        &self,
        block: block_wire_format::Block,
    ) -> Result<(), IngestionServiceError> {
        let encoded_payload = encode_cbor(&block.payload)
            .map_err(|e| IngestionServiceError::ParseError(format!("encoding payload: {e}")))?;
        let payload_size = encoded_payload.len();

        let process_id = &block.process_id;
        let stream_id = &block.stream_id;
        let block_id = &block.block_id;
        let obj_path = format!("blobs/{process_id}/{stream_id}/{block_id}");
        debug!("writing {obj_path}");

        use sqlx::types::chrono::{DateTime, FixedOffset};
        let begin_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.begin_time)
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing begin_time: {e}")))?;
        let end_time = DateTime::<FixedOffset>::parse_from_rfc3339(&block.end_time)
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing end_time: {e}")))?;
        let put_outcome = {
            let begin_put = now();
            let outcome = self
                .lake
                .blob_storage
                .put_if_absent(&obj_path, encoded_payload.into())
                .await
                .map_err(|e| {
                    IngestionServiceError::StorageError(format!(
                        "writing block to blob storage: {e}"
                    ))
                })?;
            imetric!("put_duration", "ticks", (now() - begin_put) as u64);
            outcome
        };

        debug!("recording block_id={block_id} stream_id={stream_id} process_id={process_id}");
        let begin_insert = now();
        let insert_time = sqlx::types::chrono::Utc::now();
        let result = instrument_named!(
            sqlx::query(
                "INSERT INTO blocks VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (block_id) DO NOTHING;",
            )
            .bind(block_id)
            .bind(stream_id)
            .bind(process_id)
            .bind(begin_time)
            .bind(block.begin_ticks)
            .bind(end_time)
            .bind(block.end_ticks)
            .bind(block.nb_objects)
            .bind(block.object_offset)
            .bind(payload_size as i64)
            .bind(insert_time)
            .execute(&self.lake.db_pool),
            "sql_insert_block"
        )
        .await
        .map_err(|e| IngestionServiceError::DatabaseError(format!("inserting into blocks: {e}")))?;
        imetric!("insert_duration", "ticks", (now() - begin_insert) as u64);

        // The object write (create-only) and the row insert (ON CONFLICT DO NOTHING) each
        // independently succeed or find the target already present, so there are four
        // (object, row) combinations. Each gets one log line and one counter — see
        // tasks/1465_create_only_block_write_plan.md's classification table.
        let row_inserted = result.rows_affected() > 0;
        match (put_outcome, row_inserted) {
            (PutIfAbsent::Created, true) => {
                // Normal first write — covered by the unconditional "recorded" debug! below.
            }
            (PutIfAbsent::AlreadyExists, false) => {
                // Retry, or two distinct events with identical bytes.
                warn!(
                    "duplicate block: object and row both already exist \
                     block_id={block_id} process_id={process_id} stream_id={stream_id}"
                );
                imetric!("block_object_duplicate", "count", 1_u64);
            }
            (PutIfAbsent::AlreadyExists, true) => {
                // Orphaned object healed (a prior attempt died between PUT and INSERT), or
                // the losing side of a concurrent-duplicate race.
                warn!(
                    "healed orphaned block object (row was missing) \
                     block_id={block_id} process_id={process_id} stream_id={stream_id}"
                );
                imetric!("block_orphan_object_healed", "count", 1_u64);
            }
            (PutIfAbsent::Created, false) => {
                // Row existed but object did not (object lost or deleted out from under its
                // row), or the winning side of a concurrent-duplicate race.
                debug!(
                    "recreated block object for a row that already existed \
                     block_id={block_id} process_id={process_id} stream_id={stream_id}"
                );
                imetric!("block_object_recreated", "count", 1_u64);
            }
        }
        // this measure does not benefit from a dynamic property - I just want to make sure the feature works well
        // the cost in this context should be reasonnable
        // Only count bytes that were actually stored: on the AlreadyExists arms, put_if_absent
        // rejected the write and the payload was discarded, so counting payload_size there would
        // inflate reported ingest volume by the redelivery/duplicate rate.
        if put_outcome == PutIfAbsent::Created {
            imetric!(
                "payload_size_inserted",
                "bytes",
                property_set::PropertySet::find_or_create(vec![property_set::Property::new(
                    "target",
                    "micromegas::ingestion"
                ),]),
                payload_size as u64
            );
        }
        debug!("recorded block_id={block_id} stream_id={stream_id} process_id={process_id}");

        Ok(())
    }

    /// Registers a stream whose blocks will be ingested in the transit format.
    ///
    /// **Known gap, not yet closed (AbAC Stage 5b, follow-up to #1373, §7).** Like
    /// [`Self::insert_block_typed`], this accepts any `process_id` unconditionally -- there is
    /// no check that the authenticated caller is authorized to write a stream onto that process.
    /// See that method's doc comment for the full write-side authorization gap this shares.
    #[span_fn]
    pub async fn insert_stream(&self, body: bytes::Bytes) -> Result<(), IngestionServiceError> {
        let stream_info: StreamInfo = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing StreamInfo: {e}")))?;
        info!(
            "new stream {} {:?} {:?}",
            stream_info.stream_id, &stream_info.tags, &stream_info.properties
        );
        let dependencies_metadata =
            encode_cbor(&stream_info.dependencies_metadata).map_err(|e| {
                IngestionServiceError::ParseError(format!("encoding dependencies_metadata: {e}"))
            })?;
        let objects_metadata = encode_cbor(&stream_info.objects_metadata).map_err(|e| {
            IngestionServiceError::ParseError(format!("encoding objects_metadata: {e}"))
        })?;
        let result = instrument_named!(
            sqlx::query(
                "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties, insert_time, format)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (stream_id) DO NOTHING;",
            )
            .bind(stream_info.stream_id)
            .bind(stream_info.process_id)
            .bind(dependencies_metadata)
            .bind(objects_metadata)
            .bind(&stream_info.tags)
            .bind(strip_reserved_properties(make_properties(
                &stream_info.properties,
            )))
            .bind(sqlx::types::chrono::Utc::now())
            .bind(FORMAT_TRANSIT)
            .execute(&self.lake.db_pool),
            "sql_insert_stream"
        )
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting into streams: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!(
                "duplicate stream_id={} skipped (already exists)",
                stream_info.stream_id
            );
        }
        Ok(())
    }

    /// Registers a stream produced by an OTLP ingestion path.
    ///
    /// `dependencies_metadata` and `objects_metadata` are filled with the CBOR sentinel
    /// for an empty `Vec<UserDefinedType>` so legacy decode sites continue to work.
    /// `format` distinguishes per-block dispatch downstream (e.g. `"otlp/v1/logs"`).
    /// Stream `properties` are always empty for OTel — scope and per-event attrs
    /// live on individual rows during materialization, not on the stream.
    ///
    /// Hack: piggybacking OTLP onto the transit-shaped `streams` row (with empty
    /// metadata sentinels) is expedient for two formats but won't scale. To support
    /// more formats cleanly, `dependencies_metadata`, `objects_metadata`, and `format`
    /// should be merged into a single per-format payload column.
    #[span_fn]
    pub async fn register_otel_stream(
        &self,
        stream_id: Uuid,
        process_id: Uuid,
        tags: Vec<String>,
        format: &str,
    ) -> Result<(), IngestionServiceError> {
        let result = instrument_named!(
            sqlx::query(
                "INSERT INTO streams (stream_id, process_id, dependencies_metadata, objects_metadata, tags, properties, insert_time, format)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
             ON CONFLICT (stream_id) DO NOTHING;",
            )
            .bind(stream_id)
            .bind(process_id)
            .bind(empty_transit_metadata_cbor())
            .bind(empty_transit_metadata_cbor())
            .bind(tags)
            .bind(Vec::<Property>::new())
            .bind(sqlx::types::chrono::Utc::now())
            .bind(format)
            .execute(&self.lake.db_pool),
            "sql_insert_stream"
        )
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting otel stream: {e}"))
        })?;

        if result.rows_affected() == 0 {
            debug!("duplicate otel stream_id={stream_id} skipped (already exists)");
        }
        Ok(())
    }

    /// Registers a process from the native (CBOR) ingestion path, stamping it with `audience`
    /// (AbAC Stage 5, #1373). `audience` is resolved by the caller from the authenticated
    /// credential (`AuthContext.bound_audience`); this method never trusts a client-supplied
    /// `micromegas.audience` property -- [`finalize_process_properties`] strips it.
    ///
    /// A conflicting re-registration (an existing `process_id` under a *different* audience
    /// than `audience`) is rejected with [`IngestionServiceError::AudienceConflict`] (§6) rather
    /// than silently no-op'd: `process_id` is client-chosen on this path, so a reused id under a
    /// different credential is a real, reachable case, and it is what keeps Stage 2's
    /// `MAX(audience)` per-process resolution (`ownership_rewrite.rs`) sound. An existing `NULL`
    /// audience is left alone (no retro-stamp, ever -- see the module-level design doc) since a
    /// mid-migration re-registration must not lose the process.
    #[span_fn]
    pub async fn insert_process(
        &self,
        body: bytes::Bytes,
        audience: &WriteAudience,
    ) -> Result<(), IngestionServiceError> {
        let process_info: ProcessInfo = ciborium::from_reader(body.reader())
            .map_err(|e| IngestionServiceError::ParseError(format!("parsing ProcessInfo: {e}")))?;

        let properties =
            finalize_process_properties(make_properties(&process_info.properties), audience);

        let insert_time = sqlx::types::chrono::Utc::now();
        let result = instrument_named!(
            sqlx::query(
                "INSERT INTO processes VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13) ON CONFLICT (process_id) DO NOTHING;",
            )
            .bind(process_info.process_id)
            .bind(process_info.exe)
            .bind(process_info.username)
            .bind(process_info.realname)
            .bind(process_info.computer)
            .bind(process_info.distro)
            .bind(process_info.cpu_brand)
            .bind(process_info.tsc_frequency)
            .bind(process_info.start_time)
            .bind(process_info.start_ticks)
            .bind(insert_time)
            .bind(process_info.parent_process_id)
            .bind(properties)
            .execute(&self.lake.db_pool),
            "sql_insert_process"
        )
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting into processes: {e}"))
        })?;

        if result.rows_affected() == 0 {
            self.check_process_audience_conflict(process_info.process_id, audience)
                .await?;
        }
        Ok(())
    }

    /// On a conflicting `insert_process` re-registration, enforces one audience per process
    /// (§6, AbAC Stage 5, #1373). No-op (aside from a `debug!`) when `audience` carries no
    /// audience at all, or when the existing row's audience is `NULL` or matches `audience`
    /// exactly; `Err(IngestionServiceError::AudienceConflict)` when the existing row was stamped
    /// with a *different* audience than this request carries.
    async fn check_process_audience_conflict(
        &self,
        process_id: Uuid,
        audience: &WriteAudience,
    ) -> Result<(), IngestionServiceError> {
        let Some(incoming) = audience.as_str() else {
            debug!("duplicate process_id={process_id} skipped (already exists)");
            return Ok(());
        };
        let properties: Vec<Property> = instrument_named!(
            sqlx::query_scalar("SELECT properties FROM processes WHERE process_id = $1")
                .bind(process_id)
                .fetch_one(&self.lake.db_pool),
            "sql_select_process_properties"
        )
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!(
                "reading existing process properties for conflict check: {e}"
            ))
        })?;
        let existing = properties
            .iter()
            .find(|p| p.key_str() == PROPERTY_AUDIENCE)
            .map(|p| p.value_str().to_string());
        match existing {
            Some(existing) if existing != incoming => {
                warn!(
                    "process_id={process_id} audience conflict: existing={existing:?} \
                     incoming={incoming:?} -- rejecting re-registration"
                );
                Err(IngestionServiceError::AudienceConflict {
                    process_id,
                    existing,
                    incoming: incoming.to_string(),
                })
            }
            Some(_) => {
                debug!("duplicate process_id={process_id} skipped (already exists, same audience)");
                Ok(())
            }
            None => {
                debug!(
                    "duplicate process_id={process_id} skipped (already exists, unstamped -- no retro-stamp)"
                );
                Ok(())
            }
        }
    }

    /// Registers a process originating from OTLP. Idempotent via `ON CONFLICT DO NOTHING`.
    ///
    /// `realname` is set equal to `username` (OTel has no separate "real name" concept).
    /// `parent_process_id` is always NULL — OTel has no parent-process model.
    /// `insert_time` is the server wall clock, matching the existing `insert_process` path.
    ///
    /// `audience` (AbAC Stage 5, #1373) is stamped via [`finalize_process_properties`], exactly
    /// like `insert_process`. **Same conflict guard as `insert_process`, §6, and for a
    /// confidentiality reason, not just consistency**: `processes` is a single table shared with
    /// the native path, and `insert_process` accepts a client-chosen `process_id` stamped with
    /// the caller's own audience. Because `process_id_from_resource`'s derivation formula is
    /// public, any ingestion credential can pre-register (via the native path) the exact
    /// `process_id` a victim audience's OTLP producer will later derive; without this guard the
    /// genuine producer's stream/blocks would silently land on a row stamped with the squatter's
    /// audience, leaking that audience's data to the squatter. `check_process_audience_conflict`
    /// closes that hole the same way it does for `insert_process`.
    #[span_fn]
    #[expect(clippy::too_many_arguments, reason = "OTel process identity fields")]
    pub async fn register_otel_process(
        &self,
        process_id: Uuid,
        exe: String,
        username: String,
        computer: String,
        distro: String,
        cpu_brand: String,
        tsc_frequency: i64,
        start_time: sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>,
        start_ticks: i64,
        properties: Vec<Property>,
        audience: &WriteAudience,
    ) -> Result<(), IngestionServiceError> {
        let properties = finalize_process_properties(properties, audience);
        let insert_time = sqlx::types::chrono::Utc::now();
        let result = instrument_named!(
            sqlx::query(
                "INSERT INTO processes
             (process_id, exe, username, realname, computer, distro, cpu_brand,
              tsc_frequency, start_time, start_ticks, insert_time, parent_process_id, properties)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,NULL,$12)
             ON CONFLICT (process_id) DO NOTHING;",
            )
            .bind(process_id)
            .bind(exe)
            .bind(&username)
            .bind(&username)
            .bind(computer)
            .bind(distro)
            .bind(cpu_brand)
            .bind(tsc_frequency)
            .bind(start_time)
            .bind(start_ticks)
            .bind(insert_time)
            .bind(properties)
            .execute(&self.lake.db_pool),
            "sql_insert_process"
        )
        .await
        .map_err(|e| {
            IngestionServiceError::DatabaseError(format!("inserting otel process: {e}"))
        })?;

        if result.rows_affected() == 0 {
            self.check_process_audience_conflict(process_id, audience)
                .await?;
        }
        Ok(())
    }
}
