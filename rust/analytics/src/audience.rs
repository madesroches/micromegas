//! The single `micromegas.audience` property name and SQL fragments shared by the writer
//! (`lakehouse::blocks_view`) and both enforcement prongs (`lakehouse::audience_guard`,
//! `lakehouse::ownership_rewrite`) -- and by `metadata.rs`, the JIT / per-process path (#1482
//! §1).
//!
//! A process registered under a credential that carried no audience keeps **no**
//! `micromegas.audience` property in Postgres, permanently. The default is applied where the
//! audience is *read*, not where the process is written: every read site wraps the extraction in
//! [`coalesced_audience_subselect`], so a missing property resolves to the deployment's
//! `MICROMEGAS_DEFAULT_AUDIENCE` and a `NULL` audience is unrepresentable downstream of Postgres.
//! That is what lets the materialized `audience` column be non-nullable and what lets Prong B
//! resolve every existing id to a real audience.

/// The reserved process property key an ingestion credential's audience is stamped under.
/// Re-exported here (rather than each consumer importing `micromegas_telemetry` directly) so the
/// property name stays single-sourced across the write side and both readers.
pub use micromegas_telemetry::property_names::PROPERTY_AUDIENCE as AUDIENCE_PROPERTY;

/// Builds `(SELECT value FROM unnest(<properties_expr>) WHERE key = '<AUDIENCE_PROPERTY>' LIMIT 1)`
/// -- the correlated scalar subselect that extracts a process's audience out of its
/// `micromegas_property[]` properties array. `properties_expr` is inlined as SQL text (not
/// bound), so it must be a trusted column reference, never user input.
///
/// Yields `NULL` for a process that was never stamped. Read sites want
/// [`coalesced_audience_subselect`] instead, which resolves that `NULL` to the deployment
/// default; use this bare form only where a `NULL` is meaningful on its own.
pub fn audience_subselect(properties_expr: &str) -> String {
    format!(
        "(SELECT value FROM unnest({properties_expr}) WHERE key = '{AUDIENCE_PROPERTY}' LIMIT 1)"
    )
}

/// [`audience_subselect`] wrapped in `COALESCE(..., $<param>)`: the audience of a stamped
/// process, or the deployment's `MICROMEGAS_DEFAULT_AUDIENCE` for one that was never stamped.
///
/// This is the shared shape of all three sites that read an audience out of Postgres -- the
/// `blocks` view's `data_sql`, `metadata::find_process`, and `audience_guard`'s
/// `owner_query_sql`. The default is a **bind parameter**, not interpolated: `properties_expr` is
/// a trusted column reference, but the default is operator-supplied config and must stay out of
/// the SQL text. `param` is the 1-based placeholder index the caller will bind the default to.
pub fn coalesced_audience_subselect(properties_expr: &str, param: usize) -> String {
    format!(
        "COALESCE({}, ${param})",
        audience_subselect(properties_expr)
    )
}

/// The audience assigned to anything that arrives without one. Matches
/// `micromegas_auth::policy::PUBLIC_AUDIENCE`, kept as a local copy for the same crate-boundary
/// reason [`default_audience_from_env`] duplicates the charset check.
pub const DEFAULT_AUDIENCE: &str = "public";

/// `true` if `aud` is a valid audience name: `[A-Za-z0-9_-]{1,255}`, checked in bytes.
///
/// A third copy of the same predicate, beside `micromegas_auth::policy::is_valid_audience` and
/// `micromegas_ingestion::write_audience`'s: `micromegas-analytics` does not depend on
/// `micromegas-auth`, and the lakehouse must validate this knob without acquiring that
/// dependency. Keep the copies in step if the charset ever changes.
fn is_valid_audience(aud: &str) -> bool {
    !aud.is_empty()
        && aud.len() <= 255
        && aud
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Resolves `MICROMEGAS_DEFAULT_AUDIENCE` -- the audience a never-stamped process is read as --
/// defaulting to [`DEFAULT_AUDIENCE`] when unset. Malformed ⇒ `Err`, so a typo fails startup
/// rather than silently relabelling every legacy process.
///
/// Read **once**, by `LakehouseContext`, and handed to all three read sites from there. Every
/// role that materializes or queries the six global views builds a `LakehouseContext`, and the
/// maintenance role is the one that bakes the resolved value into partitions -- so a deployment
/// that sets this knob on only some roles gets partitions labelled inconsistently. Changing it
/// is not a routine operation: already-written partitions keep the value they were materialized
/// under until they are regenerated.
///
/// Unprefixed, unlike `IsolationConfig`'s knobs: this is not a per-caller query-side setting but
/// a property of the lake's contents, and `micromegas_auth::policy::default_audience_from_env`
/// -- the other reader of this same variable, on the key-minting side -- falls back to exactly
/// this unprefixed name. Trimming and its warning match that reader, so one env value can never
/// be accepted by one role and rejected by another.
pub fn default_audience_from_env() -> anyhow::Result<String> {
    const VAR: &str = "MICROMEGAS_DEFAULT_AUDIENCE";
    let resolved = match std::env::var(VAR) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed != raw {
                micromegas_tracing::warn!(
                    "{VAR}: value {raw:?} has leading or trailing whitespace -- using {trimmed:?}"
                );
            }
            if !is_valid_audience(trimmed) {
                anyhow::bail!(
                    "{VAR}: {trimmed:?} is not a valid audience name -- must match \
                     [A-Za-z0-9_-]{{1,255}}"
                );
            }
            trimmed.to_string()
        }
        Err(_) => DEFAULT_AUDIENCE.to_owned(),
    };
    micromegas_tracing::info!("{VAR}: default audience = {resolved}");
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesced_subselect_wraps_the_bare_one_and_binds_the_default() {
        let bare = audience_subselect("processes.properties");
        let coalesced = coalesced_audience_subselect("processes.properties", 3);
        assert_eq!(coalesced, format!("COALESCE({bare}, $3)"));
        // The default is bound, never inlined: an operator-supplied label must not reach the
        // SQL text even though `properties_expr` does.
        assert!(!coalesced.contains(DEFAULT_AUDIENCE));
    }

    #[test]
    fn each_read_site_can_pick_its_own_placeholder_index() {
        // `blocks`' `data_sql` binds $1/$2 for the insert range, so its default is $3;
        // `find_process` binds $1 for the process id, so its default is $2.
        assert!(coalesced_audience_subselect("properties", 2).ends_with(", $2)"));
        assert!(coalesced_audience_subselect("processes.properties", 3).ends_with(", $3)"));
    }

    #[test]
    fn charset_matches_the_other_two_copies() {
        assert!(is_valid_audience("public"));
        assert!(is_valid_audience("team-alpha_1"));
        assert!(!is_valid_audience(""));
        assert!(!is_valid_audience("team alpha"));
        assert!(!is_valid_audience("user:alice@example.com"));
        assert!(!is_valid_audience(&"a".repeat(256)));
        assert!(is_valid_audience(&"a".repeat(255)));
    }
}
