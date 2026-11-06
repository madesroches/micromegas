//! Tests for lakehouse-modifying statement classification.

use analytics_web_srv::admin_statements::{
    AdminStatement, MUTATING_ADMIN_FUNCTIONS, classify_admin_statement,
};

#[test]
fn mutating_functions_match_in_any_form() {
    for func in MUTATING_ADMIN_FUNCTIONS {
        let upper = func.to_uppercase();
        let cases = [
            format!("SELECT * FROM {func}('a')"),
            format!("SELECT * FROM {upper}('a')"),
            format!("SELECT 1 -- {func}\nFROM t"),
            format!("SELECT /* {func} */ 1"),
            format!("SELECT * FROM \"{func}\"('a')"),
            format!("SELECT * FROM (SELECT * FROM {func}('a')) s"),
        ];
        for sql in cases {
            assert_eq!(
                classify_admin_statement(&sql),
                Some(AdminStatement::Function(func)),
                "{sql}"
            );
        }
    }
}

#[test]
fn read_only_admin_functions_are_not_matched() {
    for sql in [
        "SELECT * FROM list_query_denials()",
        "SELECT * FROM list_view_set_definitions()",
        "SELECT * FROM check_view_merge('a')",
        "SELECT * FROM list_partitions()",
    ] {
        assert_eq!(classify_admin_statement(sql), None, "{sql}");
    }
}

#[test]
fn view_ddl_is_detected() {
    for sql in [
        "CREATE MATERIALIZED VIEW v WITH (view_set = 'log_entries', retention = '1d') AS SELECT 1",
        "create or replace materialized view v with (view_set = 'log_entries') as select 1",
        "DROP MATERIALIZED VIEW v",
        "drop materialized view if exists v",
        "-- note\nDROP MATERIALIZED VIEW v",
        "/* note */ drop materialized view v",
    ] {
        assert!(
            matches!(
                classify_admin_statement(sql),
                Some(AdminStatement::ViewDdl { .. })
            ),
            "{sql}"
        );
    }
}

#[test]
fn malformed_view_ddl_fails_closed() {
    assert_eq!(
        classify_admin_statement("CREATE MATERIALIZED VIEW x"),
        Some(AdminStatement::ViewDdl { parsed: false })
    );
}

#[test]
fn other_statements_are_not_matched() {
    for sql in [
        "SELECT 1",
        "CREATE VIEW v AS SELECT 1",
        "CREATE TABLE t (a INT)",
        "SELECT * FROM log_entries LIMIT 10",
    ] {
        assert_eq!(classify_admin_statement(sql), None, "{sql}");
    }
}
