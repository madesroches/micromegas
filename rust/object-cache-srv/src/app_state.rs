use micromegas_object_cache::range_cache::RangeCache;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) cache: RangeCache,
    pub(crate) allowed_prefix: String,
}
