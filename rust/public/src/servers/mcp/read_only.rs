use datafusion::sql::sqlparser::ast::{
    Expr, ObjectName, ObjectNamePart, Query, SetExpr, Statement, TableFactor, Visit, Visitor,
};
use datafusion::sql::sqlparser::dialect::GenericDialect;
use datafusion::sql::sqlparser::parser::Parser;
use micromegas_analytics::lakehouse::view_definition::MUTATING_FUNCTIONS;
use std::ops::ControlFlow;

/// Admin-only read functions. They are not mutating, but no admin feature is reachable through
/// MCP; the admin/non-admin comparison test keeps this list complete.
pub const ADMIN_ONLY_READ_FUNCTIONS: &[&str] = &[
    "list_query_denials",
    "list_view_set_definitions",
    "check_view_merge",
];

/// Whether `name` (compared case-insensitively) is a function the MCP layer refuses to call.
pub fn is_denied_function(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MUTATING_FUNCTIONS.contains(&lower.as_str())
        || ADMIN_ONLY_READ_FUNCTIONS.contains(&lower.as_str())
}

struct Checker;

fn check_name(name: &ObjectName) -> ControlFlow<String> {
    for part in &name.0 {
        match part {
            ObjectNamePart::Identifier(ident) => {
                if is_denied_function(&ident.value) {
                    return ControlFlow::Break(format!(
                        "function '{}' is not available through MCP",
                        ident.value
                    ));
                }
            }
            ObjectNamePart::Function(_) => {
                return ControlFlow::Break("computed object names are not allowed".to_string());
            }
        }
    }
    ControlFlow::Continue(())
}

fn check_set_expr(body: &SetExpr) -> ControlFlow<String> {
    match body {
        SetExpr::Insert(_) | SetExpr::Update(_) | SetExpr::Delete(_) | SetExpr::Merge(_) => {
            ControlFlow::Break("only read-only SELECT queries are allowed".to_string())
        }
        _ => ControlFlow::Continue(()),
    }
}

impl Visitor for Checker {
    type Break = String;

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        check_set_expr(&query.body)
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        check_name(relation)
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<Self::Break> {
        match table_factor {
            TableFactor::Table { name, .. } | TableFactor::Function { name, .. } => {
                check_name(name)
            }
            _ => ControlFlow::Continue(()),
        }
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(func) = expr {
            return check_name(&func.name);
        }
        ControlFlow::Continue(())
    }
}

/// Accepts a single read-only `SELECT`-style query and rejects everything else: other statement
/// kinds, several statements, and calls to mutating or admin-only functions anywhere in the query.
pub fn check_read_only(sql: &str) -> Result<(), String> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql)
        .map_err(|e| format!("could not parse SQL: {e}"))?;
    let [statement] = statements.as_slice() else {
        return Err("exactly one statement is allowed".to_string());
    };
    if !matches!(statement, Statement::Query(_)) {
        return Err("only a single read-only SELECT query is allowed".to_string());
    }
    match statement.visit(&mut Checker) {
        ControlFlow::Break(reason) => Err(reason),
        ControlFlow::Continue(()) => Ok(()),
    }
}
