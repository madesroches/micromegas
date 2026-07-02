use anyhow::{Context, Result, anyhow, bail};

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
