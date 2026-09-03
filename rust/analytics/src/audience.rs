//! The precedence rule every audience-carrying reader follows, and the two SQL fragments
//! shared by the writer (`lakehouse::blocks_view`) and both enforcement prongs
//! (`lakehouse::audience_guard`, `lakehouse::ownership_rewrite`) -- and by `metadata.rs`, the
//! JIT / per-process path.
//!
//! **A row's own `audience` column is the authoritative label for that row.** It is the
//! authenticated fact recorded at the moment the row was written (`processes.audience`,
//! `streams.audience`, `blocks.audience`). A NULL column means the row predates this stage
//! and resolves to the deployment's `MICROMEGAS_DEFAULT_AUDIENCE`. No row's audience is ever
//! derived from another row's for any row that carries the column; a reader with no
//! `audience` column of its own still resolves through the owning process/stream row.
//!
//! `check_process_audience_conflict` (and its stream-side mirror
//! `check_stream_audience_conflict`) still governs only the `processes` (or `streams`) row it
//! re-registers.

/// `COALESCE(<qualifier>.audience, $<param>)` -- a row's own stamp, or the deployment's
/// `MICROMEGAS_DEFAULT_AUDIENCE` for a row written before this stage (a NULL column). `qualifier`
/// is inlined as SQL text, so it must be a trusted table name or alias, never user input; the
/// default stays a bind parameter, since it is operator-supplied config.
///
/// This is the shared shape of every site that reads an audience out of Postgres off a physical
/// column -- the `blocks` view's `data_sql`, `metadata::find_process`, and `audience_guard`'s
/// `owner_query_sql`. `param` is the 1-based placeholder index the caller will bind the default
/// to.
pub fn coalesced_audience_column(qualifier: &str, param: usize) -> String {
    format!("COALESCE({qualifier}.audience, ${param})")
}

/// True when `left_qualifier.audience` and `right_qualifier.audience` are both set but
/// disagree. NULL-tolerant: a NULL `audience` on either side does not count as a mismatch, so
/// a legacy row on either side of the join passes through cleanly. Both qualifiers must be
/// trusted table names/aliases, never user input.
pub fn audience_column_mismatch(left_qualifier: &str, right_qualifier: &str) -> String {
    format!(
        "{left_qualifier}.audience IS NOT NULL AND {right_qualifier}.audience IS NOT NULL \
         AND {left_qualifier}.audience <> {right_qualifier}.audience"
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

/// Resolves `MICROMEGAS_DEFAULT_AUDIENCE` -- the audience a never-stamped row is read as --
/// defaulting to [`DEFAULT_AUDIENCE`] when unset. Malformed ⇒ `Err`, so a typo fails startup
/// rather than silently relabelling every legacy row.
///
/// Read **once**, by `LakehouseContext`, and handed to all read sites from there. Every role that
/// materializes or queries the six global views builds a `LakehouseContext`, and the maintenance
/// role is the one that bakes the resolved value into partitions -- so a deployment that sets
/// this knob on only some roles gets partitions labelled inconsistently. Changing it is not a
/// routine operation: already-written partitions keep the value they were materialized under
/// until they are regenerated.
///
/// This is not a per-caller query-side setting but a property of the lake's contents. Two other
/// code readers of the same variable fall back to this same unprefixed name:
/// `micromegas_auth::policy::default_audience_from_env`, on the key-minting side, and the
/// ingestion HTTP edge's own call in `serve_ingestion`, which resolves the default a credential
/// with no bound audience is stamped with at write time.
/// Trimming and its warning match both, so one env value can never be accepted by one role and
/// rejected by another.
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
    fn coalesced_column_wraps_the_qualifier_and_binds_the_default() {
        let coalesced = coalesced_audience_column("processes", 3);
        assert_eq!(coalesced, "COALESCE(processes.audience, $3)");
        // The default is bound, never inlined: an operator-supplied label must not reach the
        // SQL text even though `qualifier` does.
        assert!(!coalesced.contains(DEFAULT_AUDIENCE));
    }

    #[test]
    fn each_read_site_can_pick_its_own_placeholder_index() {
        // `blocks`' `data_sql` binds $1/$2 for the insert range, so its default is $3;
        // `find_process` binds $1 for the process id, so its default is $2.
        assert!(coalesced_audience_column("processes", 2).ends_with(", $2)"));
        assert!(coalesced_audience_column("blocks", 3).ends_with(", $3)"));
    }

    #[test]
    fn mismatch_emits_the_expected_null_tolerant_text() {
        let mismatch = audience_column_mismatch("blocks", "streams");
        assert_eq!(
            mismatch,
            "blocks.audience IS NOT NULL AND streams.audience IS NOT NULL AND blocks.audience <> streams.audience"
        );
    }

    #[test]
    fn mismatch_qualifiers_are_not_swapped() {
        let mismatch = audience_column_mismatch("blocks", "processes");
        assert!(mismatch.starts_with("blocks.audience IS NOT NULL"));
        assert!(mismatch.contains("processes.audience IS NOT NULL"));
        assert!(mismatch.ends_with("blocks.audience <> processes.audience"));
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
