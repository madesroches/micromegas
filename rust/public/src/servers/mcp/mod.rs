//! MCP (Model Context Protocol) server over Streamable HTTP, for agents that cannot run the
//! `micromegas-query` CLI.
//!
//! The module holds the tool logic, guardrails and rendering and knows nothing about the host
//! application. The host validates the bearer token (putting the resulting
//! [`micromegas_auth::types::AuthContext`] in the request extensions) and supplies the tool
//! providers; queries are forwarded to FlightSQL with the caller's token, which stays the
//! enforcement point.

/// configuration read from `MICROMEGAS_MCP_*`
pub mod config;
/// FlightSQL access used by the query tools
pub mod flight;
/// the `ToolProvider` seam between this module and its host
pub mod provider;
/// rejection of anything but read-only queries
pub mod read_only;
/// capped markdown rendering of query results
pub mod render;
/// MCP handler
pub mod server;
/// skills served over the MCP skills extension
pub mod skills;
/// query and discovery tools
pub mod tools;

use anyhow::Result;
use axum::Router;
use config::McpConfig;
use provider::ToolProvider;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use std::sync::Arc;

/// Adds the MCP route at `config.path`. The route does not authenticate: the host must layer a
/// bearer validation middleware on the returned router.
pub fn register_routes(
    router: Router,
    config: &McpConfig,
    providers: Vec<Arc<dyn ToolProvider>>,
) -> Result<Router> {
    let server = server::McpServer::new(config, providers)?;
    // The body cap is enforced by the service while streaming (413), so no extra layer.
    // Stateless: every tool call carries its full arguments, so no per-client state is kept.
    let http_config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_allowed_hosts(config.allowed_hosts.clone())
        .with_allowed_origins(config.allowed_origins.clone())
        .with_max_request_body_bytes(config.max_request_body_bytes);
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(NeverSessionManager::default()),
        http_config,
    );
    Ok(router.route_service(&config.path, service))
}
