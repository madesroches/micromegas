//! No-DB tests of the screen tools against an in-memory store that applies the same validation
//! as the real store functions.

use analytics_web_srv::app_db::{
    CreateScreenRequest, Folder, FolderInfo, Screen, UpdateScreenRequest,
};
use analytics_web_srv::auth::ValidatedUser;
use analytics_web_srv::folders::{FolderError, FolderResult};
use analytics_web_srv::mcp_screens::{ScreenStore, ScreenToolProvider};
use analytics_web_srv::screens::{
    ScreenError, ScreenResult, ScreenSummary, parse_screen_type, validate_create_request,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use micromegas::auth::types::{AuthContext, AuthType};
use micromegas::servers::mcp::provider::{CallContext, ToolProvider};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct MemStore {
    screens: Mutex<Vec<Screen>>,
    last_creator: Mutex<Option<String>>,
}

#[async_trait]
impl ScreenStore for MemStore {
    async fn list_screens(&self, folder: Option<&str>) -> ScreenResult<Vec<ScreenSummary>> {
        Ok(self
            .screens
            .lock()
            .expect("lock")
            .iter()
            .filter(|s| folder.is_none_or(|f| s.folder_path == f))
            .map(|s| ScreenSummary {
                name: s.name.clone(),
                screen_type: s.screen_type.clone(),
                folder_path: s.folder_path.clone(),
                managed_by: s.managed_by.clone(),
                updated_at: s.updated_at,
            })
            .collect())
    }

    async fn get_screen(&self, name: &str) -> ScreenResult<Screen> {
        self.screens
            .lock()
            .expect("lock")
            .iter()
            .find(|s| s.name == name)
            .cloned()
            .ok_or_else(|| ScreenError::NotFound(name.to_string()))
    }

    async fn create_screen(
        &self,
        user: &ValidatedUser,
        request: &CreateScreenRequest,
    ) -> ScreenResult<Screen> {
        let name = validate_create_request(request)?;
        let creator = user.email.clone().unwrap_or_else(|| user.subject.clone());
        *self.last_creator.lock().expect("lock") = Some(creator.clone());
        let screen = Screen {
            name,
            screen_type: request.screen_type.clone(),
            config: request.config.clone(),
            created_by: Some(creator.clone()),
            updated_by: Some(creator),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
            managed_by: request.managed_by.clone(),
            folder_path: request.folder_path.clone(),
        };
        self.screens.lock().expect("lock").push(screen.clone());
        Ok(screen)
    }

    async fn update_screen(
        &self,
        _user: &ValidatedUser,
        name: &str,
        request: &UpdateScreenRequest,
        expected: Option<DateTime<Utc>>,
    ) -> ScreenResult<Screen> {
        let mut screens = self.screens.lock().expect("lock");
        let screen = screens
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| ScreenError::NotFound(name.to_string()))?;
        if let Some(expected) = expected
            && screen.updated_at != Some(expected)
        {
            return Err(ScreenError::Conflict("modified".to_string()));
        }
        if let Some(config) = &request.config {
            screen.config = config.clone();
        }
        if let Some(folder) = &request.folder_path {
            screen.folder_path = folder.clone();
        }
        Ok(screen.clone())
    }

    async fn delete_screen(&self, name: &str) -> ScreenResult<()> {
        let mut screens = self.screens.lock().expect("lock");
        let before = screens.len();
        screens.retain(|s| s.name != name);
        if screens.len() == before {
            return Err(ScreenError::NotFound(name.to_string()));
        }
        Ok(())
    }

    async fn list_folders(&self) -> FolderResult<Vec<FolderInfo>> {
        Ok(vec![])
    }

    async fn create_folder(&self, _user: &ValidatedUser, path: &str) -> FolderResult<Folder> {
        if path.contains("..") {
            return Err(FolderError::NotFound(path.to_string()));
        }
        Ok(Folder {
            path: path.to_string(),
        })
    }
}

fn ctx(auth_type: AuthType) -> CallContext {
    CallContext {
        auth: AuthContext {
            subject: "sub".into(),
            email: Some("a@b.c".into()),
            issuer: "i".into(),
            audience: None,
            expires_at: None,
            auth_type,
            allow_delegation: false,
            bound_audience: None,
            read_audiences: vec![],
            analytics_key_id: None,
            memberships: Arc::from([]),
        },
        authorization: "Bearer x".into(),
        client_agent: "agent".into(),
        client_ip: "127.0.0.1".into(),
    }
}

fn provider(store: Arc<MemStore>, writes: bool) -> ScreenToolProvider {
    ScreenToolProvider::new(store, writes, 4096)
}

fn names(p: &ScreenToolProvider, c: &CallContext) -> Vec<String> {
    p.tools(c).iter().map(|t| t.name.to_string()).collect()
}

fn text(r: &CallToolResult) -> String {
    match &r.content[0] {
        ContentBlock::Text(t) => t.text.clone(),
        other => panic!("unexpected {other:?}"),
    }
}

async fn call(
    p: &ScreenToolProvider,
    c: &CallContext,
    name: &str,
    args: serde_json::Value,
) -> CallToolResult {
    let args: JsonObject = args.as_object().expect("object").clone();
    p.call_tool(c, name, args).await.expect("tool")
}

const WRITES: [&str; 4] = [
    "create_folder",
    "create_screen",
    "update_screen",
    "delete_screen",
];

#[tokio::test]
async fn write_tools_absent_unless_enabled_and_oidc() {
    let store = Arc::new(MemStore::default());
    let key = ctx(AuthType::ApiKey);
    let oidc = ctx(AuthType::Oidc);
    for (enabled, c, expect_writes) in [
        (false, &oidc, false),
        (true, &key, false),
        (true, &oidc, true),
    ] {
        let p = provider(store.clone(), enabled);
        let listed = names(&p, c);
        for w in WRITES {
            assert_eq!(listed.contains(&w.to_string()), expect_writes, "{w}");
        }
        assert!(listed.contains(&"list_screens".to_string()));
        if !expect_writes {
            let r = call(&p, c, "delete_screen", serde_json::json!({"name": "x"})).await;
            assert_eq!(r.is_error, Some(true));
        }
    }
}

#[tokio::test]
async fn create_defaults_config_and_list_omits_it() {
    let store = Arc::new(MemStore::default());
    let p = provider(store.clone(), true);
    let c = ctx(AuthType::Oidc);
    let r = call(
        &p,
        &c,
        "create_screen",
        serde_json::json!({"name": "my-nb", "screen_type": "notebook"}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    let stored = store.get_screen("my-nb").await.expect("stored");
    assert_eq!(stored.config["cells"], serde_json::json!([]));
    assert_eq!(
        store.last_creator.lock().expect("lock").as_deref(),
        Some("a@b.c")
    );

    let listed = call(&p, &c, "list_screens", serde_json::json!({})).await;
    let t = text(&listed);
    assert!(t.contains("my-nb"));
    assert!(!t.contains("config") && !t.contains("cells"));

    let got = call(&p, &c, "get_screen", serde_json::json!({"name": "my-nb"})).await;
    assert!(text(&got).contains("\"config\""));
}

#[tokio::test]
async fn update_honors_expected_updated_at() {
    let store = Arc::new(MemStore::default());
    let p = provider(store.clone(), true);
    let c = ctx(AuthType::Oidc);
    call(
        &p,
        &c,
        "create_screen",
        serde_json::json!({"name": "my-nb", "screen_type": "notebook"}),
    )
    .await;
    let current = store
        .get_screen("my-nb")
        .await
        .expect("screen")
        .updated_at
        .expect("ts");

    let stale = (current - chrono::Duration::seconds(5)).to_rfc3339();
    let r = call(
        &p,
        &c,
        "update_screen",
        serde_json::json!({"name": "my-nb", "config": {"cells": [1]}, "expected_updated_at": stale}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("CONFLICT"));

    let r = call(
        &p,
        &c,
        "update_screen",
        serde_json::json!({"name": "my-nb", "config": {"cells": [1]}, "expected_updated_at": current.to_rfc3339()}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    assert_eq!(
        store.get_screen("my-nb").await.expect("screen").config["cells"],
        serde_json::json!([1])
    );

    let r = call(
        &p,
        &c,
        "update_screen",
        serde_json::json!({"name": "my-nb", "expected_updated_at": "garbage"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn invalid_inputs_map_to_is_error() {
    let store = Arc::new(MemStore::default());
    let p = provider(store, true);
    let c = ctx(AuthType::Oidc);
    for args in [
        serde_json::json!({"name": "", "screen_type": "notebook"}),
        serde_json::json!({"name": "ok", "screen_type": "bogus"}),
        serde_json::json!({"name": "ok", "screen_type": "notebook", "folder_path": "a//b"}),
    ] {
        let r = call(&p, &c, "create_screen", args).await;
        assert_eq!(r.is_error, Some(true), "{}", text(&r));
    }
    let r = call(
        &p,
        &c,
        "get_screen_type_default",
        serde_json::json!({"screen_type": "bogus"}),
    )
    .await;
    assert_eq!(r.is_error, Some(true));
    assert!(parse_screen_type("notebook").is_ok());
    let r = call(&p, &c, "get_screen", serde_json::json!({"name": "missing"})).await;
    assert_eq!(r.is_error, Some(true));
}

#[tokio::test]
async fn large_configs_are_truncated() {
    let store = Arc::new(MemStore::default());
    let p = provider(store, true);
    let c = ctx(AuthType::Oidc);
    let big = "x".repeat(10_000);
    call(
        &p,
        &c,
        "create_screen",
        serde_json::json!({"name": "big", "screen_type": "notebook", "config": {"cells": [big]}}),
    )
    .await;
    let r = call(&p, &c, "get_screen", serde_json::json!({"name": "big"})).await;
    assert!(text(&r).contains("truncated at 4096 bytes"));
}
