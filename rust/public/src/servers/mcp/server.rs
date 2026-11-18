use super::config::McpConfig;
use super::provider::{CallContext, ToolProvider};
use super::skills::Skills;
use crate::servers::http_utils::get_client_ip;
use micromegas_auth::types::AuthContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    GetSkillRequestParams, GetSkillResult, ListResourcesResult, ListSkillsResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler};
use std::sync::Arc;

/// The short rule list returned in the `initialize` result. Tool descriptions repeat the
/// critical ones because not every client shows server instructions.
pub const INSTRUCTIONS: &str = "\
Query micromegas observability data (logs, metrics, traces) with SQL. Mandatory rules:
1. Every query must bound its time range with the begin/end arguments (for example begin=1h); never use WHERE time >= ... instead.
2. Every query must have a LIMIT unless it is a small aggregate. Results are cut at a row and byte cap.
3. Call list_views, then describe_view for each view you query: its description is binding (time windows, mandatory filters).
4. Use view_instance() for single-process or single-stream queries. thread_spans and async_events have no global view.
5. Call prepare_query first when a query might scan a lot, then execute_query with the same arguments.
Only read-only SELECT queries are accepted. The micromegas-query and micromegas-notebooks skills are served over the MCP skills extension.";

struct Inner {
    providers: Vec<Arc<dyn ToolProvider>>,
    skills: Skills,
}

/// MCP handler: dispatches tools to providers and serves skills.
#[derive(Clone)]
pub struct McpServer {
    inner: Arc<Inner>,
}

impl McpServer {
    pub fn new(config: &McpConfig, providers: Vec<Arc<dyn ToolProvider>>) -> anyhow::Result<Self> {
        let skills = Skills::new(config.skills_dir.as_deref())?;
        Ok(Self {
            inner: Arc::new(Inner { providers, skills }),
        })
    }
}

/// Reads the caller's identity from the HTTP request the MCP service is handling.
fn call_context(ctx: &RequestContext<RoleServer>) -> Option<CallContext> {
    let parts = ctx.extensions.get::<http::request::Parts>()?;
    let auth = parts.extensions.get::<AuthContext>()?.clone();
    let header = |name: http::header::HeaderName| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    Some(CallContext {
        auth,
        authorization: header(http::header::AUTHORIZATION).unwrap_or_default(),
        client_agent: header(http::header::USER_AGENT).unwrap_or_else(|| "unknown".to_string()),
        client_ip: get_client_ip(&parts.headers, &parts.extensions),
    })
}

impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_skills()
                .build(),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let call = call_context(&context).ok_or_else(|| {
            ErrorData::internal_error("request carries no authenticated identity", None)
        })?;
        let tools = self
            .inner
            .providers
            .iter()
            .flat_map(|p| p.tools(&call))
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(call) = call_context(&context) else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Unauthenticated: the request carries no validated identity",
            )])
            .into());
        };
        let arguments = request.arguments.unwrap_or_default();
        for provider in &self.inner.providers {
            if let Some(result) = provider
                .call_tool(&call, &request.name, arguments.clone())
                .await
            {
                return Ok(result.into());
            }
        }
        Err(ErrorData::invalid_params(
            format!("unknown tool '{}'", request.name),
            None,
        ))
    }

    async fn list_skills(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListSkillsResult, ErrorData> {
        Ok(self.inner.skills.list_skills())
    }

    async fn get_skill(
        &self,
        request: GetSkillRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetSkillResult, ErrorData> {
        self.inner.skills.get_skill(&request.uri).ok_or_else(|| {
            ErrorData::invalid_params(format!("unknown skill {}", request.uri), None)
        })
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(
            self.inner.skills.resources(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        self.inner
            .skills
            .read_resource(&request.uri)
            .map(Into::into)
            .ok_or_else(|| ErrorData::resource_not_found(request.uri, None))
    }
}
