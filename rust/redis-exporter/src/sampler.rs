//! Metric sampling and emission. Modeled on
//! `object-cache-srv/src/saturation_monitor.rs`: pure emit functions driven
//! by a loop in `main.rs`, so tests can drive one round without the loop.
//!
//! `imetric!`/`fmetric!` require literal metric names (per-call-site static
//! metadata), so each metric is an explicit call; dynamic strings appear
//! only as property values, interned to `'static` (bounded cardinality:
//! command names, latency events, db indices, one instance name).
use anyhow::{Context, Result};
use micromegas::tracing::intern_string::intern_string;
use micromegas::tracing::prelude::*;
use micromegas::tracing::property_set::{Property, PropertySet};

use crate::cli::MetricsPreset;
use crate::info_parser::ParsedInfo;

/// Properties attached to every metric: `instance` plus user-supplied pairs.
pub fn build_base_properties(
    target_name: &str,
    extra: &[(String, String)],
) -> &'static PropertySet {
    let mut props = vec![Property::new("instance", intern_string(target_name))];
    for (key, value) in extra {
        props.push(Property::new(intern_string(key), intern_string(value)));
    }
    PropertySet::find_or_create(props)
}

/// Base properties + one extra tag (e.g. `command=get`, `db=0`).
fn with_extra(base: &'static PropertySet, name: &'static str, value: &str) -> &'static PropertySet {
    let mut props = base.get_properties().to_vec();
    props.push(Property::new(name, intern_string(value)));
    PropertySet::find_or_create(props)
}

/// One full sampling round. `now_unix_secs` is passed in for testability
/// (used to derive the RDB last-save age).
pub async fn sample_once(
    conn: &mut redis::aio::ConnectionManager,
    preset: MetricsPreset,
    props: &'static PropertySet,
    now_unix_secs: u64,
) -> Result<()> {
    // `INFO all` also returns the Commandstats section needed by `full`;
    // plain `INFO` (default sections) is enough below that and cheaper.
    let raw: String = if preset >= MetricsPreset::Full {
        redis::cmd("INFO").arg("all").query_async(conn).await
    } else {
        redis::cmd("INFO").query_async(conn).await
    }
    .context("INFO")?;
    let info = ParsedInfo::parse(&raw);
    emit_info_metrics(&info, props, now_unix_secs);

    if preset >= MetricsPreset::Extended {
        let slowlog_len: u64 = redis::cmd("SLOWLOG")
            .arg("LEN")
            .query_async(conn)
            .await
            .context("SLOWLOG LEN")?;
        imetric!("redis_slowlog_length", "count", props, slowlog_len);
    }

    if preset >= MetricsPreset::Full {
        emit_command_stats(&info, props);
        // Rows of (event, unix timestamp, latest ms, max ms).
        let latency: Vec<(String, u64, u64, u64)> = redis::cmd("LATENCY")
            .arg("LATEST")
            .query_async(conn)
            .await
            .context("LATENCY LATEST")?;
        for (event, _timestamp, latest_ms, max_ms) in &latency {
            let tagged = with_extra(props, "event", event);
            imetric!("redis_latency_latest_ms", "ms", tagged, *latest_ms);
            imetric!("redis_latency_max_ms", "ms", tagged, *max_ms);
        }
    }
    Ok(())
}

/// Core metrics from one INFO response. Absent or unparseable fields are
/// skipped (Redis versions differ), never fatal.
pub fn emit_info_metrics(info: &ParsedInfo, props: &'static PropertySet, now_unix_secs: u64) {
    if let Some(v) = info.get_u64("uptime_in_seconds") {
        imetric!("redis_uptime_seconds", "seconds", props, v);
    }
    if let Some(v) = info.get_u64("connected_clients") {
        imetric!("redis_connected_clients", "count", props, v);
    }
    if let Some(v) = info.get_u64("blocked_clients") {
        imetric!("redis_blocked_clients", "count", props, v);
    }
    if let Some(v) = info.get_u64("used_memory") {
        imetric!("redis_used_memory_bytes", "bytes", props, v);
    }
    if let Some(v) = info.get_u64("used_memory_rss") {
        imetric!("redis_used_memory_rss_bytes", "bytes", props, v);
    }
    if let Some(v) = info.get_u64("maxmemory") {
        imetric!("redis_maxmemory_bytes", "bytes", props, v);
    }
    if let Some(v) = info.get_f64("mem_fragmentation_ratio") {
        fmetric!("redis_mem_fragmentation_ratio", "ratio", props, v);
    }
    if let Some(v) = info.get_u64("instantaneous_ops_per_sec") {
        imetric!("redis_ops_per_sec", "ops_per_sec", props, v);
    }
    if let Some(v) = info.get_u64("total_commands_processed") {
        imetric!("redis_total_commands_processed", "count", props, v);
    }
    if let Some(v) = info.get_u64("total_net_input_bytes") {
        imetric!("redis_total_net_input_bytes", "bytes", props, v);
    }
    if let Some(v) = info.get_u64("total_net_output_bytes") {
        imetric!("redis_total_net_output_bytes", "bytes", props, v);
    }
    if let Some(v) = info.get_u64("keyspace_hits") {
        imetric!("redis_keyspace_hits", "count", props, v);
    }
    if let Some(v) = info.get_u64("keyspace_misses") {
        imetric!("redis_keyspace_misses", "count", props, v);
    }
    if let Some(v) = info.get_u64("expired_keys") {
        imetric!("redis_expired_keys", "count", props, v);
    }
    if let Some(v) = info.get_u64("evicted_keys") {
        imetric!("redis_evicted_keys", "count", props, v);
    }
    if let Some(v) = info.get_u64("connected_slaves") {
        imetric!("redis_connected_replicas", "count", props, v);
    }
    if let Some(v) = info.get_u64("master_repl_offset") {
        imetric!("redis_master_repl_offset", "count", props, v);
    }
    if info.get_str("role") == Some("slave") {
        if let Some(v) = info.get_u64("slave_repl_offset") {
            imetric!("redis_replica_repl_offset", "count", props, v);
        }
        if let Some(v) = info.get_u64("master_last_io_seconds_ago") {
            imetric!("redis_master_last_io_seconds_ago", "seconds", props, v);
        }
        if let Some(status) = info.get_str("master_link_status") {
            let up = u64::from(status == "up");
            imetric!("redis_master_link_up", "count", props, up);
        }
    }
    if let Some(v) = info.get_u64("rdb_changes_since_last_save") {
        imetric!("redis_rdb_changes_since_last_save", "count", props, v);
    }
    if let Some(t) = info.get_u64("rdb_last_save_time") {
        imetric!(
            "redis_rdb_last_save_age_seconds",
            "seconds",
            props,
            now_unix_secs.saturating_sub(t)
        );
    }
    if info.get_u64("aof_enabled") == Some(1)
        && let Some(v) = info.get_u64("aof_current_size")
    {
        imetric!("redis_aof_current_size_bytes", "bytes", props, v);
    }
    for entry in info.keyspace() {
        let tagged = with_extra(props, "db", &entry.db.to_string());
        imetric!("redis_db_keys", "count", tagged, entry.keys);
        imetric!("redis_db_expires", "count", tagged, entry.expires);
    }
}

/// Per-command metrics from the Commandstats section (present in `INFO all`).
pub fn emit_command_stats(info: &ParsedInfo, props: &'static PropertySet) {
    for stat in info.command_stats() {
        let tagged = with_extra(props, "command", &stat.name);
        imetric!("redis_command_calls", "count", tagged, stat.calls);
        imetric!("redis_command_usec", "us", tagged, stat.usec);
        fmetric!(
            "redis_command_usec_per_call",
            "us",
            tagged,
            stat.usec_per_call
        );
    }
}
