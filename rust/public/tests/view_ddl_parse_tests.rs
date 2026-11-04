//! `authorize_view_ddl` unit tests -- no DB, no network: the admin gate is a plain boolean check
//! on `CallerContext`.

use micromegas::servers::view_ddl::authorize_view_ddl;
use micromegas_analytics::lakehouse::read_scope::{CallerContext, IsolationConfig, ReadScope};
use std::sync::Arc;

fn admin_caller() -> CallerContext {
    CallerContext {
        read_scope: ReadScope::All,
        is_admin: true,
        isolation_config: Arc::new(IsolationConfig::default()),
        identity: Some("admin".to_string()),
        grant_selectors: Arc::from([]),
    }
}

fn non_admin_caller() -> CallerContext {
    CallerContext {
        is_admin: false,
        ..admin_caller()
    }
}

#[test]
fn authorize_view_ddl_allows_admin() {
    assert!(authorize_view_ddl(&admin_caller()).is_ok());
}

#[test]
fn authorize_view_ddl_denies_non_admin() {
    let status = authorize_view_ddl(&non_admin_caller()).unwrap_err();
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
}
