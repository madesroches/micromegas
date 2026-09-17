//! `ViewRegistry`: a shared `Arc<ViewFactory>` (the code-driven base plus every DDL-defined global
//! view) rebuilt from `lakehouse_view_set_definitions` on a timer, so `telemetry-maintenance-srv`
//! and `flight-sql-srv` both start reading/materializing a new or changed view set without a
//! restart. Shaped after `QueryDenyList`: a Postgres-backed store, an immutable compiled snapshot
//! behind a lock, a `refresh`/`reload` that logs and meters its own failures, and
//! `spawn_refresh_task`.

use super::{
    session_configurator::SessionConfigurator,
    view_definition::{build_sql_batch_view, validate_view_definition},
    view_definition_store::{ViewDefinitionRow, ViewDefinitionStore},
    view_factory::ViewFactory,
};
use anyhow::Result;
use datafusion::execution::runtime_env::RuntimeEnv;
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_tracing::intern_string::intern_string;
use micromegas_tracing::prelude::*;
use micromegas_tracing::property_set::{Property, PropertySet};
use std::collections::HashSet;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// `{prefix}`-less env var: how often `spawn_refresh_task` calls `reload()`, and the bound on
/// cross-replica propagation of a definition change made on a different node (the node that
/// executed the DDL statement reloads inline instead, see `rust/public/src/servers/view_ddl.rs`).
pub const MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS: &str =
    "MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS";
/// Default value of [`MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`].
pub const DEFAULT_REFRESH_SECONDS: u64 = 60;

fn refresh_interval() -> Duration {
    let secs = std::env::var(MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REFRESH_SECONDS)
        .max(1);
    Duration::from_secs(secs)
}

fn failure_tags(view_set_name: &str) -> &'static PropertySet {
    PropertySet::find_or_create(vec![Property::new(
        "view_set_name",
        intern_string(view_set_name),
    )])
}

/// Hashes the `(view_set_name, updated_at)` tuples of `rows`, in the order given (already
/// `(update_group, view_set_name)`-ordered by `ViewDefinitionStore::list`) -- what `reload()`
/// compares against `loaded_digest` to short-circuit a steady-state tick to one `SELECT`.
fn digest_of(rows: &[ViewDefinitionRow]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for row in rows {
        row.view_set_name.hash(&mut hasher);
        row.updated_at
            .timestamp_nanos_opt()
            .unwrap_or(0)
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// Builds a factory from an explicit row set instead of `store.list()`, so a caller already
/// holding rows inside an open transaction (the DDL executor's dependent-protection check) can
/// validate against them without a second, pool-backed read that would not see its own
/// uncommitted write. `reload()` calls this too, after its own `store.list()`. `async` because
/// building each row's `SqlBatchView` is itself `async` and needs `runtime`/`lake`/
/// `session_configurator` to plan the extract query.
///
/// Builds incrementally, in the order `rows` is given (`(update_group, view_set_name)`): `let mut
/// factory = (**base).clone();` then, per row, build and validate its `SqlBatchView` against
/// `Arc::new(factory.clone())` -- the built-ins plus every already-folded-in row -- and only then
/// `factory.add_global_view(...)`. A row that fails to parse, build, or validate is **skipped, not
/// fatal**: logged and metered, and its name collected into the returned failed-set, so one broken
/// definition cannot take out every other view set.
pub async fn build_factory(
    base: &Arc<ViewFactory>,
    rows: &[ViewDefinitionRow],
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    session_configurator: Arc<dyn SessionConfigurator>,
) -> Result<(Arc<ViewFactory>, Vec<String>)> {
    let mut factory = (**base).clone();
    let mut failed = Vec::new();
    for row in rows {
        let name = row.view_set_name.clone();
        let def = match row.clone().into_definition() {
            Ok(def) => def,
            Err(e) => {
                warn!("view_registry: '{name}' failed to parse, skipping: {e:#}");
                imetric!(
                    "view_definition_load_failure",
                    "count",
                    failure_tags(&name),
                    1_u64
                );
                failed.push(name);
                continue;
            }
        };
        let factory_so_far = Arc::new(factory.clone());
        let view = match build_sql_batch_view(
            &def,
            runtime.clone(),
            lake.clone(),
            factory_so_far.clone(),
            session_configurator.clone(),
        )
        .await
        {
            Ok(view) => view,
            Err(e) => {
                warn!("view_registry: '{name}' failed to build, skipping: {e:#}");
                imetric!(
                    "view_definition_load_failure",
                    "count",
                    failure_tags(&name),
                    1_u64
                );
                failed.push(name);
                continue;
            }
        };
        if let Err(e) = validate_view_definition(
            &def,
            &view,
            &factory_so_far,
            runtime.clone(),
            lake.clone(),
            session_configurator.clone(),
        )
        .await
        {
            warn!("view_registry: '{name}' failed validation, skipping: {e:#}");
            imetric!(
                "view_definition_load_failure",
                "count",
                failure_tags(&name),
                1_u64
            );
            failed.push(name);
            continue;
        }
        factory.add_global_view(Arc::new(view));
    }
    Ok((Arc::new(factory), failed))
}

/// A shared `Arc<ViewFactory>` (base + N DDL-defined `SqlBatchView`s) rebuilt from
/// `lakehouse_view_set_definitions` on a timer. See the module doc comment.
pub struct ViewRegistry {
    base: Arc<ViewFactory>,
    store: Arc<dyn ViewDefinitionStore>,
    runtime: Arc<RuntimeEnv>,
    lake: Arc<DataLakeConnection>,
    session_configurator: Arc<dyn SessionConfigurator>,
    current: RwLock<Arc<ViewFactory>>,
    loaded_digest: RwLock<Option<u64>>,
    failed_view_sets: RwLock<Vec<String>>,
    /// Held across all of `reload()` -- list, build, and swap -- not just the swap. Without this,
    /// a periodic reload that listed rows before a DDL-triggered reload could block behind the
    /// DDL's own reload and then swap the pre-mutation factory back in over the DDL executor's
    /// post-commit one, a lost update that would stay live for up to
    /// `MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`. Mirrors `QueryDenyList::write_lock`'s
    /// rationale.
    reload_mutex: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ViewRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewRegistry")
            .field(
                "loaded_views",
                &self.current.read().expect("lock").get_global_views().len(),
            )
            .field(
                "failed_view_sets",
                &self.failed_view_sets.read().expect("lock").len(),
            )
            .finish()
    }
}

impl ViewRegistry {
    pub fn new(
        base: Arc<ViewFactory>,
        store: Arc<dyn ViewDefinitionStore>,
        runtime: Arc<RuntimeEnv>,
        lake: Arc<DataLakeConnection>,
        session_configurator: Arc<dyn SessionConfigurator>,
    ) -> Self {
        Self {
            current: RwLock::new(base.clone()),
            base,
            store,
            runtime,
            lake,
            session_configurator,
            loaded_digest: RwLock::new(None),
            failed_view_sets: RwLock::new(Vec::new()),
            reload_mutex: tokio::sync::Mutex::new(()),
        }
    }

    /// The current snapshot: the base factory before the first successful `reload()`, or the base
    /// plus every definition that built and validated on the last successful build afterward.
    pub fn current(&self) -> Arc<ViewFactory> {
        self.current.read().expect("lock").clone()
    }

    /// Names skipped by the last build, for introspection/tests.
    pub fn failed_view_sets(&self) -> Vec<String> {
        self.failed_view_sets.read().expect("lock").clone()
    }

    /// Thin wrapper over [`build_factory`] supplying this registry's `base`/`runtime`/`lake`/
    /// `session_configurator`.
    pub async fn build_from_rows(
        &self,
        rows: &[ViewDefinitionRow],
    ) -> Result<(Arc<ViewFactory>, Vec<String>)> {
        build_factory(
            &self.base,
            rows,
            self.runtime.clone(),
            self.lake.clone(),
            self.session_configurator.clone(),
        )
        .await
    }

    /// The dependent-protection refusal: builds a factory from `pre_rows` and one from
    /// `post_rows`, then refuses with a named error if any definition present in `post_rows` --
    /// including the row just written -- both fails to build against `post_rows` and was not
    /// already in the pre-mutation failed set. Exact rather than textual: it uses the real
    /// planner, so it catches a removed column as readily as a removed view set.
    pub async fn check_dependents_survive(
        &self,
        pre_rows: &[ViewDefinitionRow],
        post_rows: &[ViewDefinitionRow],
    ) -> Result<()> {
        let (_, pre_failed) = self.build_from_rows(pre_rows).await?;
        let (_, post_failed) = self.build_from_rows(post_rows).await?;
        let pre_failed_set: HashSet<&str> = pre_failed.iter().map(String::as_str).collect();
        let newly_broken: Vec<&String> = post_failed
            .iter()
            .filter(|name| !pre_failed_set.contains(name.as_str()))
            .collect();
        if !newly_broken.is_empty() {
            anyhow::bail!(
                "this change would break the following view definition(s), which built \
                 successfully before it: {}",
                newly_broken
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Ok(())
    }

    /// Refreshes `current()` from `lakehouse_view_set_definitions`.
    ///
    /// 0. Acquires `reload_mutex` and holds it through every step below.
    /// 1. `store.list()` -> rows ordered by `(update_group, view_set_name)`.
    /// 2. Hashes the `(view_set_name, updated_at)` tuples; if unchanged from `loaded_digest`
    ///    **and** the previous build's failed set was empty, returns -- one `SELECT`, no
    ///    planning. A digest match after a failed build does *not* short-circuit, so a row that
    ///    failed for a transient reason is retried every tick.
    /// 3. `build_factory(...)`.
    /// 4. A row that fails to build or validate is skipped, not fatal (see [`build_factory`]).
    /// 5. Swaps `current`, updates `loaded_digest` and `failed_view_sets`.
    pub async fn reload(&self) -> Result<()> {
        let _guard = self.reload_mutex.lock().await;
        let rows = self.store.list().await?;
        let digest = digest_of(&rows);
        let previous_build_was_healthy = self.failed_view_sets.read().expect("lock").is_empty();
        if previous_build_was_healthy && Some(digest) == *self.loaded_digest.read().expect("lock") {
            return Ok(());
        }
        let (factory, failed) = self.build_from_rows(&rows).await?;
        *self.current.write().expect("lock") = factory;
        *self.loaded_digest.write().expect("lock") = Some(digest);
        *self.failed_view_sets.write().expect("lock") = failed;
        Ok(())
    }

    /// Spawns the background task that calls [`Self::reload`] once immediately and then on every
    /// [`MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS`] tick, until `shutdown` resolves. A failed
    /// `reload()` is `warn!`-logged here and discarded -- the previous snapshot is kept (the same
    /// fail-open treatment `QueryDenyList::spawn_refresh_task` gives a failed periodic refresh).
    pub fn spawn_refresh_task(
        self: Arc<Self>,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) {
        tokio::spawn(async move {
            let mut shutdown = Box::pin(shutdown);
            loop {
                if let Err(e) = self.reload().await {
                    warn!("view_registry: reload failed, keeping previous snapshot: {e:#}");
                }
                tokio::select! {
                    _ = &mut shutdown => break,
                    _ = tokio::time::sleep(refresh_interval()) => {}
                }
            }
        });
    }
}
