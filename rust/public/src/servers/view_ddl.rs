//! Admin gate for `CREATE`/`DROP MATERIALIZED VIEW`. The statement parser itself lives in
//! `micromegas_analytics::lakehouse::view_ddl` (so the `check_view_merge` UDTF can reuse it);
//! `ViewDdl` and `parse_view_ddl` are re-exported here for the Flight SQL front end.

use micromegas_analytics::lakehouse::read_scope::CallerContext;
pub use micromegas_analytics::lakehouse::view_ddl::{ViewDdl, parse_view_ddl};
use tonic::Status;

/// The same admin gate as the ten admin-gated lakehouse UDTFs/UDFs
/// (`query.rs::register_lakehouse_functions`'s `if lakehouse_admin` block), pulled out as a
/// standalone function since this path never builds a caller session context to register a UDTF
/// against in the first place. Also load-bearing beyond the usual reason: a DDL view's queries
/// are planned and materialized under `CallerContext::maintenance()` (`ReadScope::All`), so an
/// author sees every audience regardless of their own read scope.
pub fn authorize_view_ddl(caller: &CallerContext) -> Result<(), Status> {
    if !caller.is_admin {
        return Err(Status::permission_denied(
            "CREATE/DROP MATERIALIZED VIEW requires admin privileges",
        ));
    }
    Ok(())
}
