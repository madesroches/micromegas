use micromegas_object_cache::range_cache::RangeCache;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) cache: RangeCache,
    /// Prefixes a request key may fall under. Empty = allow every key; this is
    /// only reachable via `--allow-all-prefixes`, since the server refuses to
    /// start with an empty list otherwise.
    pub(crate) allowed_prefixes: Vec<String>,
}
