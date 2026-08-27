//! Shared JSONB extraction helpers, used by both the `jsonb_each` / `jsonb_array_elements`
//! UDTFs and the list-returning scalar UDFs (`jsonb_entries`, `jsonb_elements`,
//! `jsonb_path_elements`).
//!
//! These functions are pure — no DataFusion array/UDF plumbing — and return `Ok(None)` for a
//! "wrong shape" input (e.g. an object passed where an array is expected) rather than an error,
//! so callers can decide whether that should surface as NULL (the scalar UDFs) or as an error
//! (the UDTFs, which map `Ok(None)` back to their existing error text).

use datafusion::common::Result;
use datafusion::error::DataFusionError;
use jsonb::RawJsonb;
use jsonb::jsonpath::JsonPath;

/// `(key, value)` pairs, one per JSONB object field or array element.
pub type EntryList = Vec<(String, Vec<u8>)>;

/// Object → `(field name, value)`; array → `(index as string, value)`. `Ok(None)` if the input
/// is a JSON scalar (neither an object nor an array).
pub fn object_or_array_entries(jsonb_bytes: &[u8]) -> Result<Option<EntryList>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.object_each() {
        Ok(Some(entries)) => {
            return Ok(Some(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, v.as_ref().to_vec()))
                    .collect(),
            ));
        }
        Ok(None) => {}
        Err(e) => return Err(DataFusionError::External(e.into())),
    }
    match jsonb.array_values() {
        Ok(Some(values)) => Ok(Some(
            values
                .into_iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v.as_ref().to_vec()))
                .collect(),
        )),
        Ok(None) => Ok(None),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

/// Array → elements. `Ok(None)` if the input is not an array.
pub fn array_elements(jsonb_bytes: &[u8]) -> Result<Option<Vec<Vec<u8>>>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.array_values() {
        Ok(Some(values)) => Ok(Some(
            values.into_iter().map(|v| v.as_ref().to_vec()).collect(),
        )),
        Ok(None) => Ok(None),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

/// All matches of a parsed JSONPath, as separate values — not wrapped in one JSONB array (that's
/// `select_array_by_path`, used by `jsonb_path_query`). An empty result means zero matches, not
/// an error.
pub fn path_select_all<'a>(jsonb_bytes: &[u8], path: &'a JsonPath<'a>) -> Result<Vec<Vec<u8>>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.select_by_path(path) {
        Ok(values) => Ok(values.into_iter().map(|v| v.as_ref().to_vec()).collect()),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}
