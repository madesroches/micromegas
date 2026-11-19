//! Screen and folder tools for the MCP route.
//!
//! The tools call the same store functions as the HTTP handlers in `screens.rs`/`folders.rs`,
//! so name, folder and screen-type validation live in one place.

use crate::app_db::{CreateScreenRequest, Folder, FolderInfo, Screen, UpdateScreenRequest};
use crate::auth::ValidatedUser;
use crate::folders::{self, FolderError, FolderResult};
use crate::screen_types::ScreenType;
use crate::screens::{self, ScreenError, ScreenResult, ScreenSummary};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use micromegas::auth::types::AuthType;
use micromegas::servers::mcp::provider::{
    CallContext, ToolProvider, error_result, input_schema, text_result,
};
use micromegas::servers::mcp::render::truncate_text;
use micromegas::tracing::prelude::*;
use rmcp::model::{CallToolResult, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sqlx::PgPool;
use std::sync::Arc;

/// Storage behind the screen tools. A trait so the tools can be tested without a database.
#[async_trait]
pub trait ScreenStore: Send + Sync {
    async fn list_screens(&self, folder_path: Option<&str>) -> ScreenResult<Vec<ScreenSummary>>;
    async fn get_screen(&self, name: &str) -> ScreenResult<Screen>;
    async fn create_screen(
        &self,
        user: &ValidatedUser,
        request: &CreateScreenRequest,
    ) -> ScreenResult<Screen>;
    async fn update_screen(
        &self,
        user: &ValidatedUser,
        name: &str,
        request: &UpdateScreenRequest,
        expected_updated_at: Option<DateTime<Utc>>,
    ) -> ScreenResult<Screen>;
    async fn delete_screen(&self, name: &str) -> ScreenResult<()>;
    async fn list_folders(&self) -> FolderResult<Vec<FolderInfo>>;
    async fn create_folder(&self, user: &ValidatedUser, path: &str) -> FolderResult<Folder>;
}

/// The store backed by the web app database.
pub struct PgScreenStore {
    pool: PgPool,
}

impl PgScreenStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ScreenStore for PgScreenStore {
    async fn list_screens(&self, folder_path: Option<&str>) -> ScreenResult<Vec<ScreenSummary>> {
        screens::fetch_screen_summaries(&self.pool, folder_path).await
    }

    async fn get_screen(&self, name: &str) -> ScreenResult<Screen> {
        screens::fetch_screen(&self.pool, name).await
    }

    async fn create_screen(
        &self,
        user: &ValidatedUser,
        request: &CreateScreenRequest,
    ) -> ScreenResult<Screen> {
        screens::insert_screen(&self.pool, user, request).await
    }

    async fn update_screen(
        &self,
        user: &ValidatedUser,
        name: &str,
        request: &UpdateScreenRequest,
        expected_updated_at: Option<DateTime<Utc>>,
    ) -> ScreenResult<Screen> {
        screens::modify_screen(&self.pool, user, name, request, expected_updated_at).await
    }

    async fn delete_screen(&self, name: &str) -> ScreenResult<()> {
        screens::remove_screen(&self.pool, name).await
    }

    async fn list_folders(&self) -> FolderResult<Vec<FolderInfo>> {
        folders::list_folder_infos(&self.pool).await
    }

    async fn create_folder(&self, user: &ValidatedUser, path: &str) -> FolderResult<Folder> {
        folders::insert_folder(&self.pool, user, path).await
    }
}

#[derive(Deserialize)]
struct ListScreensArgs {
    folder_path: Option<String>,
}

#[derive(Deserialize)]
struct NameArgs {
    name: String,
}

#[derive(Deserialize)]
struct ScreenTypeArgs {
    screen_type: String,
}

#[derive(Deserialize)]
struct CreateFolderArgs {
    path: String,
}

#[derive(Deserialize)]
struct CreateScreenArgs {
    name: String,
    screen_type: String,
    config: Option<serde_json::Value>,
    folder_path: Option<String>,
}

#[derive(Deserialize)]
struct UpdateScreenArgs {
    name: String,
    config: Option<serde_json::Value>,
    folder_path: Option<String>,
    expected_updated_at: Option<String>,
}

fn parse_args<T: DeserializeOwned>(arguments: JsonObject) -> Result<T, Box<CallToolResult>> {
    serde_json::from_value(serde_json::Value::Object(arguments))
        .map_err(|e| Box::new(error_result(format!("invalid arguments: {e}"))))
}

fn json_result<T: serde::Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(text) => text_result(text),
        Err(e) => error_result(format!("could not serialize result: {e}")),
    }
}

fn screen_error(err: ScreenError) -> CallToolResult {
    if let ScreenError::Database(e) = &err {
        error!("mcp screen tool database error: {e}");
    }
    error_result(err.describe())
}

fn folder_error(err: FolderError) -> CallToolResult {
    if let FolderError::Database(e) = &err {
        error!("mcp folder tool database error: {e}");
    }
    error_result(err.describe())
}

const NOTEBOOK_POINTER: &str = "Read the micromegas-notebooks skill for the notebook config \
    format, and start from get_screen_type_default.";

const WRITE_TOOLS: &[&str] = &[
    "create_folder",
    "create_screen",
    "update_screen",
    "delete_screen",
];

/// Screens and folders, as managed in the web app. Write tools exist only when enabled and only
/// for callers authenticated with an OIDC identity: an analytics key has no user to attribute a
/// change to.
pub struct ScreenToolProvider {
    store: Arc<dyn ScreenStore>,
    writes_enabled: bool,
    max_bytes: usize,
}

impl ScreenToolProvider {
    pub fn new(store: Arc<dyn ScreenStore>, writes_enabled: bool, max_bytes: usize) -> Self {
        Self {
            store,
            writes_enabled,
            max_bytes,
        }
    }

    fn can_write(&self, ctx: &CallContext) -> bool {
        self.writes_enabled && ctx.auth.auth_type == AuthType::Oidc
    }

    fn log_mutation(&self, ctx: &CallContext, tool: &str, target: &str) {
        info!(
            "mcp {tool} target={target} subject={} agent={}",
            ctx.auth.subject, ctx.client_agent
        );
    }
}

#[async_trait]
impl ToolProvider for ScreenToolProvider {
    fn tools(&self, ctx: &CallContext) -> Vec<Tool> {
        let read = || ToolAnnotations::new().read_only(true).open_world(false);
        let mut tools = vec![
            Tool::new(
                "list_screen_types",
                "List the screen types that can be created, with descriptions.",
                input_schema(serde_json::json!({}), &[]),
            )
            .with_annotations(read()),
            Tool::new(
                "get_screen_type_default",
                format!("Default config of a screen type, a starting point for create_screen. {NOTEBOOK_POINTER}"),
                input_schema(
                    serde_json::json!({"screen_type": {"type": "string"}}),
                    &["screen_type"],
                ),
            )
            .with_annotations(read()),
            Tool::new(
                "list_screens",
                "List saved screens (notebooks) with name, type, folder, managed_by and \
                 updated_at, without their config. Use get_screen for the config.",
                input_schema(
                    serde_json::json!({"folder_path": {"type": "string", "description": "Only screens in this folder and its subfolders."}}),
                    &[],
                ),
            )
            .with_annotations(read()),
            Tool::new(
                "get_screen",
                "Get a screen with its full config JSON. Notebook cells hold SQL you can reuse \
                 through prepare_query/execute_query.",
                input_schema(serde_json::json!({"name": {"type": "string"}}), &["name"]),
            )
            .with_annotations(read()),
            Tool::new(
                "list_folders",
                "List the screen folder tree with screen and subfolder counts.",
                input_schema(serde_json::json!({}), &[]),
            )
            .with_annotations(read()),
        ];
        if self.can_write(ctx) {
            let write = || ToolAnnotations::new().read_only(false).destructive(false);
            tools.push(
                Tool::new(
                    "create_folder",
                    "Create a folder (idempotent).",
                    input_schema(serde_json::json!({"path": {"type": "string"}}), &["path"]),
                )
                .with_annotations(write().idempotent(true)),
            );
            tools.push(
                Tool::new(
                    "create_screen",
                    format!("Create a screen; config defaults to the screen type's default. {NOTEBOOK_POINTER}"),
                    input_schema(
                        serde_json::json!({
                            "name": {"type": "string"},
                            "screen_type": {"type": "string"},
                            "config": {"type": "object"},
                            "folder_path": {"type": "string"}
                        }),
                        &["name", "screen_type"],
                    ),
                )
                .with_annotations(write()),
            );
            tools.push(
                Tool::new(
                    "update_screen",
                    format!(
                        "Replace a screen's config and/or move it. Pass expected_updated_at \
                         (the updated_at from list_screens, or from get_screen when its \
                         config was not truncated) to refuse the update if someone else \
                         changed the screen meanwhile. {NOTEBOOK_POINTER}"
                    ),
                    input_schema(
                        serde_json::json!({
                            "name": {"type": "string"},
                            "config": {"type": "object"},
                            "folder_path": {"type": "string"},
                            "expected_updated_at": {"type": "string", "description": "RFC 3339"}
                        }),
                        &["name"],
                    ),
                )
                .with_annotations(write().destructive(true).idempotent(true)),
            );
            tools.push(
                Tool::new(
                    "delete_screen",
                    "Delete one screen. This cannot be undone.",
                    input_schema(serde_json::json!({"name": {"type": "string"}}), &["name"]),
                )
                .with_annotations(ToolAnnotations::new().read_only(false).destructive(true)),
            );
        }
        tools
    }

    async fn call_tool(
        &self,
        ctx: &CallContext,
        name: &str,
        arguments: JsonObject,
    ) -> Option<CallToolResult> {
        if WRITE_TOOLS.contains(&name) && !self.can_write(ctx) {
            return Some(error_result(format!(
                "tool '{name}' is not available: screen writes are disabled or the caller is \
                 not authenticated with an OIDC identity"
            )));
        }
        let user = ValidatedUser::from(&ctx.auth);
        Some(match name {
            "list_screen_types" => {
                let types: Vec<_> = ScreenType::all().iter().map(|t| t.info()).collect();
                json_result(&types)
            }
            "get_screen_type_default" => match parse_args::<ScreenTypeArgs>(arguments) {
                Ok(a) => match screens::parse_screen_type(&a.screen_type) {
                    Ok(t) => json_result(&t.default_config()),
                    Err(e) => screen_error(e),
                },
                Err(r) => *r,
            },
            "list_screens" => match parse_args::<ListScreensArgs>(arguments) {
                Ok(a) => match self.store.list_screens(a.folder_path.as_deref()).await {
                    Ok(list) => json_result(&list),
                    Err(e) => screen_error(e),
                },
                Err(r) => *r,
            },
            "get_screen" => match parse_args::<NameArgs>(arguments) {
                Ok(a) => match self.store.get_screen(&a.name).await {
                    Ok(screen) => match serde_json::to_string_pretty(&screen) {
                        Ok(text) => text_result(truncate_text(&text, self.max_bytes)),
                        Err(e) => error_result(format!("could not serialize screen: {e}")),
                    },
                    Err(e) => screen_error(e),
                },
                Err(r) => *r,
            },
            "list_folders" => match self.store.list_folders().await {
                Ok(list) => json_result(&list),
                Err(e) => folder_error(e),
            },
            "create_folder" => match parse_args::<CreateFolderArgs>(arguments) {
                Ok(a) => match self.store.create_folder(&user, &a.path).await {
                    Ok(folder) => {
                        self.log_mutation(ctx, name, &folder.path);
                        json_result(&folder)
                    }
                    Err(e) => folder_error(e),
                },
                Err(r) => *r,
            },
            "create_screen" => match parse_args::<CreateScreenArgs>(arguments) {
                Ok(a) => self.create_screen(ctx, &user, a).await,
                Err(r) => *r,
            },
            "update_screen" => match parse_args::<UpdateScreenArgs>(arguments) {
                Ok(a) => self.update_screen(ctx, &user, a).await,
                Err(r) => *r,
            },
            "delete_screen" => match parse_args::<NameArgs>(arguments) {
                Ok(a) => match self.store.delete_screen(&a.name).await {
                    Ok(()) => {
                        self.log_mutation(ctx, name, &a.name);
                        text_result(format!("deleted screen '{}'", a.name))
                    }
                    Err(e) => screen_error(e),
                },
                Err(r) => *r,
            },
            _ => return None,
        })
    }
}

impl ScreenToolProvider {
    async fn create_screen(
        &self,
        ctx: &CallContext,
        user: &ValidatedUser,
        args: CreateScreenArgs,
    ) -> CallToolResult {
        let screen_type = match screens::parse_screen_type(&args.screen_type) {
            Ok(t) => t,
            Err(e) => return screen_error(e),
        };
        let request = CreateScreenRequest {
            name: args.name,
            screen_type: args.screen_type,
            config: args.config.unwrap_or_else(|| screen_type.default_config()),
            // Left unset: the tools never touch `managed_by`.
            managed_by: None,
            folder_path: args.folder_path.unwrap_or_default(),
        };
        match self.store.create_screen(user, &request).await {
            Ok(screen) => {
                self.log_mutation(ctx, "create_screen", &screen.name);
                json_result(&ScreenSummary::from(screen))
            }
            Err(e) => screen_error(e),
        }
    }

    async fn update_screen(
        &self,
        ctx: &CallContext,
        user: &ValidatedUser,
        args: UpdateScreenArgs,
    ) -> CallToolResult {
        let expected = match args
            .expected_updated_at
            .as_deref()
            .map(DateTime::parse_from_rfc3339)
        {
            None => None,
            Some(Ok(t)) => Some(t.with_timezone(&Utc)),
            Some(Err(e)) => {
                return error_result(format!("invalid expected_updated_at: {e}"));
            }
        };
        let request = UpdateScreenRequest {
            config: args.config,
            managed_by: None,
            folder_path: args.folder_path,
        };
        match self
            .store
            .update_screen(user, &args.name, &request, expected)
            .await
        {
            Ok(screen) => {
                self.log_mutation(ctx, "update_screen", &screen.name);
                json_result(&ScreenSummary::from(screen))
            }
            Err(e) => screen_error(e),
        }
    }
}
