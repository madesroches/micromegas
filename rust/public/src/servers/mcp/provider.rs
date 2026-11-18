use async_trait::async_trait;
use micromegas_auth::types::AuthContext;
use rmcp::model::{CallToolResult, JsonObject, Tool};

/// What a tool provider knows about the caller of one MCP request.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// Identity validated by the host before the request reached the MCP service.
    pub auth: AuthContext,
    /// The full `Authorization` header value, forwarded to FlightSQL which validates it again.
    pub authorization: String,
    /// `User-Agent` of the MCP client, `unknown` when absent.
    pub client_agent: String,
    pub client_ip: String,
}

/// A group of tools served by the MCP route. The host mounts one or more providers, so a host
/// that cannot offer a capability (e.g. screen management without the app database) leaves
/// that provider out.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    /// Tools visible to this caller.
    fn tools(&self, ctx: &CallContext) -> Vec<Tool>;

    /// Runs `name`; `None` when this provider does not own the tool.
    async fn call_tool(
        &self,
        ctx: &CallContext,
        name: &str,
        arguments: JsonObject,
    ) -> Option<CallToolResult>;
}

/// Builds the JSON schema object of a tool's input.
pub fn input_schema(
    properties: serde_json::Value,
    required: &[&str],
) -> std::sync::Arc<JsonObject> {
    let mut schema = JsonObject::new();
    schema.insert("type".to_string(), "object".into());
    schema.insert("properties".to_string(), properties);
    schema.insert("required".to_string(), serde_json::json!(required));
    std::sync::Arc::new(schema)
}

pub fn text_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::success(vec![rmcp::model::ContentBlock::text(text)])
}

pub fn error_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![rmcp::model::ContentBlock::text(text)])
}
