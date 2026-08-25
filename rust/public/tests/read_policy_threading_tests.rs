//! Threading tests for the authorization seam (#1369, AbAC Stage 1): exercises
//! `FlightSqlServiceImpl` through the real `AuthService`/tonic stack (a genuine TCP listener, a
//! real `tonic::transport::Server`, and a real Flight SQL client), rather than calling its
//! handler methods directly, so that a regression in tonic's request-extension propagation --
//! the mechanism this whole seam rests on -- fails loudly instead of silently.
//!
//! `OwnershipRewrite` (#1370, AbAC Stage 2) now consumes the resolved `ReadScope`, but every
//! query here is a trivial `SELECT 1`/`SELECT 1 AS one` that never scans a `MaterializedView`, so
//! none of these tests observe a filtered query result either way -- they inject a recording stub
//! `ReadPolicy` -- the same seam a store-backed policy will occupy later -- and assert on what it
//! was called with. `start_server`'s `ViewFactory` still registers `processes`/`streams`
//! (Design §2 of `tasks/1370_ownership_rewrite_plan.md`), which `make_session_context` now
//! requires for every resolved `ReadScope::Audiences` caller regardless of what the query touches.

use anyhow::{Result, anyhow};
use arrow_flight::error::FlightError;
use arrow_flight::flight_service_client::FlightServiceClient;
use arrow_flight::flight_service_server::FlightServiceServer;
use arrow_flight::sql::CommandStatementIngest;
use arrow_flight::sql::client::FlightSqlServiceClient;
use async_trait::async_trait;
use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::datatypes::Schema;
use futures::TryStreamExt;
use micromegas::servers::flight_sql_service_impl::FlightSqlServiceImpl;
use micromegas_analytics::lakehouse::blocks_view::BlocksView;
use micromegas_analytics::lakehouse::lakehouse_context::LakehouseContext;
use micromegas_analytics::lakehouse::partition_cache::NullPartitionProvider;
use micromegas_analytics::lakehouse::processes_view::make_processes_view;
use micromegas_analytics::lakehouse::read_scope::IsolationConfig;
use micromegas_analytics::lakehouse::runtime::make_runtime_env;
use micromegas_analytics::lakehouse::session_configurator::NoOpSessionConfigurator;
use micromegas_analytics::lakehouse::streams_view::make_streams_view;
use micromegas_analytics::lakehouse::view_factory::ViewFactory;
use micromegas_auth::api_key::{ApiKeyAuthProvider, parse_key_ring};
use micromegas_auth::policy::{AudienceReadPolicy, ReadPolicy, ReadableAudiences};
use micromegas_auth::tower::AuthService;
use micromegas_auth::types::{
    AuthContext, AuthProvider, AuthType, ProviderUnavailable, RequestParts,
};
use micromegas_ingestion::data_lake_connection::DataLakeConnection;
use micromegas_telemetry::blob_storage::BlobStorage;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tonic::Code;
use tonic::transport::{Channel, Server};
use tower::ServiceBuilder;
use tower::layer::layer_fn;

/// An offline (no live DB, no network object store) `LakehouseContext`, matching
/// `rust/analytics/tests/lakehouse_admin_gate_test.rs`'s harness -- planning a trivial query like
/// `SELECT 1` never touches either.
async fn make_offline_lakehouse_context() -> Arc<LakehouseContext> {
    let db_pool = sqlx::PgPool::connect_lazy("postgres://user:pass@127.0.0.1:1/db")
        .expect("connect_lazy should not touch the network");
    let object_store: Arc<dyn object_store::ObjectStore> =
        Arc::new(object_store::memory::InMemory::new());
    let blob_storage = Arc::new(BlobStorage::new(
        object_store,
        object_store::path::Path::from("lakehouse"),
    ));
    let lake = Arc::new(DataLakeConnection::new(db_pool, blob_storage));
    let runtime = Arc::new(make_runtime_env().expect("make_runtime_env"));
    Arc::new(LakehouseContext::new(lake, runtime).expect("LakehouseContext::new"))
}

/// Builds a `ViewFactory` registering real `processes`/`streams` global views (mirroring
/// `default_view_factory`'s construction of them, and the same pattern
/// `analytics/tests/thread_spans_ordering_db_test.rs` uses), rather than
/// `lakehouse_admin_gate_test.rs`'s `ViewFactory::new(vec![])`. `OwnershipRewrite` (#1370, AbAC
/// Stage 2) requires `processes`/`streams` to be registered for every `ReadScope::Audiences`
/// caller (Design §2 of `tasks/1370_ownership_rewrite_plan.md`) -- every test in this file
/// resolves that scope via an auth provider, so without this fixture `make_session_context` would
/// fail before any SQL is planned. `SqlBatchView::new` only *plans* its transform query
/// (`ctx.sql(...)`, never executed), so the offline, `connect_lazy` lakehouse this file already
/// uses is sufficient.
async fn make_view_factory_with_processes_and_streams(
    lakehouse: &LakehouseContext,
) -> Arc<ViewFactory> {
    let blocks_view =
        Arc::new(BlocksView::new(lakehouse.default_audience()).expect("BlocksView::new"));
    let processes_view = Arc::new(
        make_processes_view(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await
        .expect("make_processes_view"),
    );
    let streams_view = Arc::new(
        make_streams_view(
            lakehouse.runtime().clone(),
            lakehouse.lake().clone(),
            Arc::new(ViewFactory::new(vec![blocks_view.clone()])),
        )
        .await
        .expect("make_streams_view"),
    );
    Arc::new(ViewFactory::new(vec![
        processes_view,
        streams_view,
        blocks_view,
    ]))
}

/// Poll the given address until a TCP connection succeeds or the timeout elapses.
async fn wait_for_server_ready(addr: SocketAddr, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return;
        }
        if start.elapsed() > timeout {
            panic!("server did not become ready within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Starts a real `FlightSqlServiceImpl` behind a real `AuthService` layer, on a real TCP
/// listener. Returns the address it is listening on.
async fn start_server(
    auth_provider: Option<Arc<dyn AuthProvider>>,
    read_policy: Arc<dyn ReadPolicy>,
) -> SocketAddr {
    let lakehouse = make_offline_lakehouse_context().await;
    let part_provider = Arc::new(NullPartitionProvider {});
    let view_factory = make_view_factory_with_processes_and_streams(&lakehouse).await;
    let session_configurator = Arc::new(NoOpSessionConfigurator);
    let admin_principal_possible = auth_provider.as_ref().is_none_or(|p| p.can_grant_admin());
    let svc = FlightServiceServer::new(FlightSqlServiceImpl::new(
        lakehouse,
        part_provider,
        view_factory,
        session_configurator,
        read_policy,
        Arc::new(IsolationConfig::default()),
        admin_principal_possible,
    ));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding listener");
    let addr = listener.local_addr().expect("getting local addr");

    tokio::spawn(async move {
        let stream = async_stream::stream! {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => yield Ok(stream),
                    Err(e) => yield Err(e),
                }
            }
        };
        let layer = ServiceBuilder::new()
            .layer(layer_fn(move |inner| AuthService {
                inner,
                auth_provider: auth_provider.clone(),
            }))
            .into_inner();
        Server::builder()
            .layer(layer)
            .add_service(svc)
            .serve_with_incoming(stream)
            .await
            .expect("server failed");
    });

    wait_for_server_ready(addr, Duration::from_secs(2)).await;
    addr
}

async fn connect(addr: SocketAddr) -> FlightSqlServiceClient<Channel> {
    let channel = Channel::builder(format!("http://{addr}").parse().expect("parsing uri"))
        .connect()
        .await
        .expect("connecting to server");
    FlightSqlServiceClient::new_from_inner(FlightServiceClient::new(channel))
}

/// An `ApiKeyAuthProvider` seeded with one key, named `name`. API keys carry
/// `allow_delegation: true`, which is what the hole-#2 test needs: an OIDC caller with
/// mismatched attribution is rejected before scope resolution ever runs
/// (`validate_and_resolve_user_attribution_grpc`), so only a delegating credential can reach
/// the resolver with client-claimed attribution that diverges from the authenticated identity.
fn api_key_provider(name: &str, key: &str) -> Arc<dyn AuthProvider> {
    let keyring = parse_key_ring(&format!(r#"[{{"name": "{name}", "key": "{key}"}}]"#))
        .expect("parsing keyring");
    Arc::new(ApiKeyAuthProvider::new(keyring))
}

/// Returns `Ok(status.code())` when `result` is a tonic-status-carrying `FlightError`; panics
/// on any other error shape or on success -- every test using this expects the RPC itself to
/// fail with a specific gRPC status, not merely "fail somehow".
fn expect_status_code<T: std::fmt::Debug>(result: std::result::Result<T, FlightError>) -> Code {
    match result {
        Err(FlightError::Tonic(status)) => status.code(),
        Err(other) => panic!("expected a tonic Status error, got: {other:?}"),
        Ok(value) => panic!("expected the RPC to fail, got: {value:?}"),
    }
}

/// A `ReadPolicy` that always fails -- the only way to exercise the resolver's `Err` branch,
/// since the shipped `AudienceReadPolicy` cannot fail.
#[derive(Debug)]
struct FailingReadPolicy {
    unavailable: bool,
}

#[async_trait]
impl ReadPolicy for FailingReadPolicy {
    async fn resolve(&self, _caller: &AuthContext) -> Result<ReadableAudiences> {
        if self.unavailable {
            Err(ProviderUnavailable(anyhow!("key store outage")).into())
        } else {
            Err(anyhow!("resolution denied"))
        }
    }
}

/// Records every `AuthContext` it was called with, alongside the `ReadableAudiences` it
/// returned -- a deterministic function of the caller's `subject` (not a fixed constant), so
/// that comparing the resolved scope across two calls is a meaningful assertion rather than a
/// tautology.
#[derive(Debug, Default)]
struct RecordingReadPolicy {
    calls: Mutex<Vec<(AuthContext, ReadableAudiences)>>,
}

impl RecordingReadPolicy {
    fn calls(&self) -> Vec<(AuthContext, ReadableAudiences)> {
        self.calls.lock().expect("lock").clone()
    }
}

#[async_trait]
impl ReadPolicy for RecordingReadPolicy {
    async fn resolve(&self, caller: &AuthContext) -> Result<ReadableAudiences> {
        let resolved = ReadableAudiences::new(Arc::from([format!("subject:{}", caller.subject)]));
        self.calls
            .lock()
            .expect("lock")
            .push((caller.clone(), resolved.clone()));
        Ok(resolved)
    }
}

/// An `AuthProvider` stub returning a fixed `AuthContext` carrying `groups` -- neither
/// `ApiKeyAuthProvider` (no groups claim at all) nor a real `OidcAuthProvider` (needs a live
/// JWKS) can exercise a groups-bearing caller without much heavier test infrastructure.
#[derive(Debug)]
struct GroupsAuthProvider {
    groups: Vec<String>,
}

#[async_trait]
impl AuthProvider for GroupsAuthProvider {
    async fn validate_request(&self, _parts: &dyn RequestParts) -> Result<AuthContext> {
        Ok(AuthContext {
            subject: "groups-caller".to_string(),
            email: Some("groups-caller@example.com".to_string()),
            issuer: "test-issuer".to_string(),
            audience: None,
            expires_at: None,
            auth_type: AuthType::Oidc,
            is_admin: false,
            allow_delegation: false,
            bound_audience: None,
            read_audiences: vec![],
            groups: self.groups.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fail-closed resolution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provider_unavailable_maps_to_status_unavailable() {
    let auth_provider = api_key_provider("test", "secret");
    let policy = Arc::new(FailingReadPolicy { unavailable: true });
    let addr = start_server(Some(auth_provider), policy).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    let info = client
        .execute("SELECT 1".to_string(), None)
        .await
        .expect("get_flight_info_statement itself does not resolve a scope");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");

    let result = client.do_get(ticket).await;
    assert_eq!(
        expect_status_code(result),
        Code::Unavailable,
        "a ProviderUnavailable error must map to Status::unavailable, never a scope"
    );
}

#[tokio::test]
async fn other_policy_error_maps_to_status_permission_denied() {
    let auth_provider = api_key_provider("test", "secret");
    let policy = Arc::new(FailingReadPolicy { unavailable: false });
    let addr = start_server(Some(auth_provider), policy).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    let info = client
        .execute("SELECT 1".to_string(), None)
        .await
        .expect("get_flight_info_statement itself does not resolve a scope");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");

    let result = client.do_get(ticket).await;
    assert_eq!(
        expect_status_code(result),
        Code::PermissionDenied,
        "a non-ProviderUnavailable error must map to Status::permission_denied, never a scope"
    );
}

/// Same failing policy on the prepared-statement path (`do_action_create_prepared_statement`),
/// closing hole #1 with the same fail-closed guarantee as `do_get`.
#[tokio::test]
async fn prepared_statement_path_also_fails_closed() {
    let auth_provider = api_key_provider("test", "secret");
    let policy = Arc::new(FailingReadPolicy { unavailable: true });
    let addr = start_server(Some(auth_provider), policy).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    let result = client.prepare("SELECT 1".to_string(), None).await;
    assert_eq!(expect_status_code(result), Code::Unavailable);
}

// ---------------------------------------------------------------------------
// Hole #2: ReadScope must derive from AuthContext, never from claimed attribution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_scope_resolves_from_auth_context_not_claimed_attribution() {
    let auth_provider = api_key_provider("delegating-key", "secret");
    let policy = Arc::new(RecordingReadPolicy::default());
    let addr = start_server(Some(auth_provider), policy.clone()).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());
    // A client-claimed attribution naming a different principal than the authenticated
    // credential. Only representable with a delegating (API-key) credential: an OIDC caller
    // with mismatched attribution is rejected with permission_denied by
    // `validate_and_resolve_user_attribution_grpc` before scope resolution ever runs.
    client.set_header("x-user-id", "attacker@example.com");
    client.set_header("x-user-email", "attacker@example.com");

    let info = client
        .execute("SELECT 1".to_string(), None)
        .await
        .expect("execute");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");
    client.do_get(ticket).await.expect("do_get");

    let calls = policy.calls();
    assert_eq!(calls.len(), 1, "expected exactly one resolve() call");
    let (auth_ctx, _) = &calls[0];
    assert_eq!(
        auth_ctx.subject, "delegating-key",
        "ReadPolicy must resolve from the authenticated AuthContext"
    );
    assert_ne!(
        auth_ctx.subject, "attacker@example.com",
        "ReadPolicy must never resolve from client-claimed attribution"
    );
}

// ---------------------------------------------------------------------------
// Prepared statement vs. do_get scope equality
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prepared_statement_resolves_the_same_scope_as_do_get() {
    let auth_provider = api_key_provider("same-key", "secret");
    let policy = Arc::new(RecordingReadPolicy::default());
    let addr = start_server(Some(auth_provider), policy.clone()).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    client
        .prepare("SELECT 1".to_string(), None)
        .await
        .expect("prepare");

    let info = client
        .execute("SELECT 1".to_string(), None)
        .await
        .expect("execute");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");
    client.do_get(ticket).await.expect("do_get");

    let calls = policy.calls();
    assert_eq!(calls.len(), 2, "expected one resolve() call per RPC path");
    let (prepared_ctx, prepared_scope) = &calls[0];
    let (do_get_ctx, do_get_scope) = &calls[1];
    assert_eq!(prepared_ctx.subject, do_get_ctx.subject);
    assert_eq!(
        prepared_scope, do_get_scope,
        "prepared-statement and do_get must resolve the same scope for the same credentials"
    );
}

// ---------------------------------------------------------------------------
// Extension survives the stack (load-bearing: tonic must propagate request extensions)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_context_with_groups_survives_the_real_tonic_stack() {
    let auth_provider: Arc<dyn AuthProvider> = Arc::new(GroupsAuthProvider {
        groups: vec!["team-a".to_string(), "team-b".to_string()],
    });
    let policy = Arc::new(RecordingReadPolicy::default());
    let addr = start_server(Some(auth_provider), policy.clone()).await;
    let mut client = connect(addr).await;
    // GroupsAuthProvider ignores the bearer token's content, but AuthService still requires the
    // header to be present to invoke validate_request at all.
    client.set_token("irrelevant".to_string());

    let info = client
        .execute("SELECT 1".to_string(), None)
        .await
        .expect("execute");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");
    client.do_get(ticket).await.expect("do_get");

    let calls = policy.calls();
    assert_eq!(calls.len(), 1);
    let (auth_ctx, _) = &calls[0];
    assert_eq!(
        auth_ctx.groups,
        vec!["team-a".to_string(), "team-b".to_string()],
        "the AuthContext observed by the handler, via request.extensions(), must be the one \
         AuthService inserted -- including the groups field -- proving tonic propagated the \
         extension all the way from the tower layer to the FlightSqlServiceImpl handler"
    );
}

// ---------------------------------------------------------------------------
// No behavior change: an unconfigured deployment still resolves a scope
// ---------------------------------------------------------------------------

/// An unconfigured deployment (audience-grants env var unset) resolves a scope through the real
/// `AudienceReadPolicy::from_env` -- not an error, not a crash -- and a query's results are
/// unaffected: `SELECT 1 AS one` never scans a `MaterializedView`, so `OwnershipRewrite` (#1370,
/// AbAC Stage 2) has nothing to filter here even though it is now registered and active for this
/// resolved `ReadScope::Audiences` caller; `do_get` must still succeed and return the same row it
/// would without this seam at all.
#[tokio::test]
async fn unconfigured_deployment_resolves_a_scope_and_query_results_are_unaffected() {
    let auth_provider = api_key_provider("test", "secret");
    let policy: Arc<dyn ReadPolicy> = Arc::new(
        AudienceReadPolicy::from_env("MICROMEGAS_1369_THREADING_TESTS_UNSET").expect("from_env"),
    );
    let addr = start_server(Some(auth_provider), policy).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    let info = client
        .execute("SELECT 1 AS one".to_string(), None)
        .await
        .expect("execute must succeed under an unconfigured (default) policy");
    let ticket = info.endpoint[0].ticket.clone().expect("ticket");
    let flight_stream = client
        .do_get(ticket)
        .await
        .expect("do_get must succeed under an unconfigured (default) policy");
    let batches: Vec<_> = flight_stream
        .try_collect()
        .await
        .expect("collecting batches");
    let total_rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total_rows, 1, "SELECT 1 must still return exactly one row");
}

// ---------------------------------------------------------------------------
// bulk_ingest admin gate (AbAC Stage 5, #1373): non-admin callers must be rejected
// ---------------------------------------------------------------------------

/// `do_put_statement_ingest`'s `is_admin` check (AbAC Stage 5, #1373) rejects a non-admin
/// caller before it ever reaches `bulk_ingest`, so this needs no live Postgres or object
/// store: an `ApiKeyAuthProvider` credential is always non-admin (see `api_key.rs`), and a
/// single empty (zero-row, zero-column) record batch is enough to reach the check -- the
/// gate denies before any ingestion work happens. A genuinely empty stream (no batches at
/// all) never reaches the server: `FlightDataEncoderBuilder` only emits a message once it
/// sees a schema or a first batch, so the command/descriptor would never be sent.
#[tokio::test]
async fn bulk_ingest_denies_non_admin_caller() {
    let auth_provider = api_key_provider("test", "secret");
    let policy = Arc::new(RecordingReadPolicy::default());
    let addr = start_server(Some(auth_provider), policy).await;
    let mut client = connect(addr).await;
    client.set_token("secret".to_string());

    let command = CommandStatementIngest {
        table: "processes".to_string(),
        ..Default::default()
    };
    let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
    let result = client
        .execute_ingest(command, futures::stream::once(async { Ok(batch) }))
        .await;

    assert_eq!(
        expect_status_code(result),
        Code::PermissionDenied,
        "a non-admin caller must be rejected by the bulk_ingest admin gate"
    );
}
