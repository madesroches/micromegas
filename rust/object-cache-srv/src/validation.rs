use anyhow::{Context, Result, anyhow, bail};

pub(crate) fn validate_key(key: &str, allowed_prefixes: &[String]) -> Result<()> {
    if key.is_empty() {
        bail!("empty key");
    }
    if key.starts_with('/') {
        bail!("key must not start with /");
    }
    if key.split('/').any(|seg| seg == "..") {
        bail!("key must not contain ..");
    }
    // An empty list means allow-all. This is only reachable via
    // `--allow-all-prefixes`; the server refuses to start with an empty list
    // otherwise (see object_cache_srv.rs), so this is never a fail-open default.
    if allowed_prefixes.is_empty() {
        return Ok(());
    }
    // Admit the key if it matches any prefix on the equal-or-`{p}/`-boundary
    // rule, so `blobs` admits `blobs` and `blobs/x` but not `blobs-secret/y`.
    let allowed = allowed_prefixes
        .iter()
        .any(|p| key == p || key.starts_with(&format!("{p}/")));
    if !allowed {
        bail!("key {key} is outside allowed prefixes {allowed_prefixes:?}");
    }
    Ok(())
}

pub(crate) fn parse_range_header(
    header_value: &str,
    file_size: u64,
) -> Result<std::ops::Range<u64>> {
    let value = header_value
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow!("invalid Range header: {header_value}"))?;
    let (start_str, end_str) = value
        .split_once('-')
        .ok_or_else(|| anyhow!("invalid Range header format: {header_value}"))?;
    let start: u64 = start_str.parse().with_context(|| "parsing range start")?;
    if end_str.is_empty() {
        // Open-ended range (`bytes=<start>-`): read from `start` to EOF. An
        // offset exactly at EOF is a legitimate zero-length read in
        // `object_store::GetRange::Offset` semantics, so allow `start == file_size`
        // to yield an empty range rather than rejecting it. Note `end > file_size`
        // (i.e. `start > file_size`) is left for the caller's OutOfBounds→416 path.
        Ok(start..file_size)
    } else {
        let end = end_str
            .parse::<u64>()
            .with_context(|| "parsing range end")?
            .checked_add(1)
            .ok_or_else(|| anyhow!("range end overflow in Range header: {header_value}"))?;
        // Reject inverted/degenerate explicit ranges (e.g. `bytes=100-50`): an
        // empty or backwards range cannot produce a valid 206 Content-Range.
        if start >= end {
            bail!("invalid Range header: start {start} not before end {end}");
        }
        Ok(start..end)
    }
}

pub(crate) fn is_not_found(e: &anyhow::Error) -> bool {
    if let Some(os_err) = e.downcast_ref::<object_store::Error>() {
        matches!(os_err, object_store::Error::NotFound { .. })
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::validate_key;

    fn prefixes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn rejects_structurally_invalid_keys() {
        let allowed = prefixes(&["blobs"]);
        assert!(validate_key("", &allowed).is_err());
        assert!(validate_key("/blobs/x", &allowed).is_err());
        assert!(validate_key("blobs/../secret", &allowed).is_err());
        // Traversal is rejected even in allow-all mode.
        assert!(validate_key("a/../b", &[]).is_err());
    }

    #[test]
    fn admits_key_under_any_configured_prefix() {
        let allowed = prefixes(&["blobs", "views"]);
        assert!(validate_key("blobs/p/s/b", &allowed).is_ok());
        assert!(validate_key("views/v/x.parquet", &allowed).is_ok());
        // A key equal to a prefix is admitted.
        assert!(validate_key("blobs", &allowed).is_ok());
        assert!(validate_key("views", &allowed).is_ok());
    }

    #[test]
    fn rejects_key_outside_all_prefixes() {
        let allowed = prefixes(&["blobs", "views"]);
        assert!(validate_key("secret/x", &allowed).is_err());
        assert!(validate_key("blob/x", &allowed).is_err());
    }

    #[test]
    fn enforces_segment_boundary_per_prefix() {
        let allowed = prefixes(&["blobs"]);
        // `blobs-secret` shares a textual prefix but not a path boundary.
        assert!(validate_key("blobs-secret/y", &allowed).is_err());
        assert!(validate_key("blobsx", &allowed).is_err());
    }

    #[test]
    fn empty_list_allows_all_valid_keys() {
        // Reachable only via --allow-all-prefixes; still enforces structure.
        assert!(validate_key("anything/goes/here", &[]).is_ok());
        assert!(validate_key("secret/x", &[]).is_ok());
    }
}
