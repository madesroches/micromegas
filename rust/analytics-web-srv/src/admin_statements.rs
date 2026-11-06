//! Classification of SQL statements that modify the lakehouse.
//!
//! The web server refuses these on the standard query endpoint so that content written by
//! someone else (a notebook) cannot run them with a viewing admin's token.

use micromegas::servers::view_ddl::parse_view_ddl;

/// A statement that modifies the lakehouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminStatement {
    /// View DDL (`CREATE [OR REPLACE] / DROP MATERIALIZED VIEW`). `parsed` is false when the text
    /// looked like view DDL but could not be parsed.
    ViewDdl { parsed: bool },
    /// A call to a mutating admin function, by name.
    Function(&'static str),
}

/// Admin functions that change lakehouse state. Read-only admin functions are deliberately absent.
pub const MUTATING_ADMIN_FUNCTIONS: &[&str] = &[
    "retire_partitions",
    "retire_partition_by_metadata",
    "retire_partition_by_file",
    "materialize_partitions",
    "regenerate_partitions",
    "deny_queries",
    "remove_query_denial",
];

/// Returns the lakehouse-modifying statement `sql` contains, if any.
///
/// DDL detection reuses the FlightSQL parser so both agree on what counts as DDL; a parse error
/// is treated as DDL (fail closed). Functions use a lower-cased substring match: it can
/// over-block (a string literal naming a function) but never under-blocks, whatever the
/// comments, quoting or whitespace.
pub fn classify_admin_statement(sql: &str) -> Option<AdminStatement> {
    match parse_view_ddl(sql) {
        Ok(None) => {}
        Ok(Some(_)) => return Some(AdminStatement::ViewDdl { parsed: true }),
        Err(_) => return Some(AdminStatement::ViewDdl { parsed: false }),
    }
    let sql_lower = sql.to_lowercase();
    MUTATING_ADMIN_FUNCTIONS
        .iter()
        .find(|&func| sql_lower.contains(func))
        .map(|func| AdminStatement::Function(func))
}

impl AdminStatement {
    /// Message returned to the client when the standard endpoint refuses this statement.
    pub fn refusal_message(&self) -> String {
        const FIX: &str = "run it from an admin page, or, as an admin, enable admin mode on a notebook from the user menu";
        match self {
            AdminStatement::ViewDdl { parsed: true } => {
                format!("View DDL modifies the lakehouse; {FIX}")
            }
            AdminStatement::ViewDdl { parsed: false } => format!(
                "The statement could not be parsed as view DDL and is refused as if it modified the lakehouse; {FIX}"
            ),
            AdminStatement::Function(name) => {
                format!("'{name}' modifies the lakehouse; {FIX}")
            }
        }
    }
}
