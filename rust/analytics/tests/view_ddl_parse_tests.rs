//! `parse_view_ddl` unit tests -- no DB, no network: parsing is pure.

use micromegas_analytics::lakehouse::log_stats_view::log_stats_ddl_text;
use micromegas_analytics::lakehouse::log_stats_view::log_stats_view_definition;
use micromegas_analytics::lakehouse::view_ddl::{ViewDdl, parse_view_ddl};

const CREATE_SQL: &str = "CREATE MATERIALIZED VIEW my_view WITH (\
    extract_query = $$SELECT 1 as x, arrow_cast('a', 'Dictionary(Int32, Utf8)') as audience$$, \
    count_src_query = $$SELECT 0 as count FROM blocks WHERE insert_time >= '{begin}' AND insert_time < '{end}'$$, \
    merge_partitions_query = $$SELECT * FROM {source}$$, \
    update_group = 9000, \
    time_column = 'x' \
)";

#[test]
fn plain_select_is_not_ddl() {
    assert!(parse_view_ddl("SELECT 1").unwrap().is_none());
    assert!(
        parse_view_ddl("  select * from log_entries")
            .unwrap()
            .is_none()
    );
}

#[test]
fn create_view_without_materialized_is_not_ddl() {
    assert!(
        parse_view_ddl("CREATE VIEW foo AS SELECT 1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn create_table_is_not_ddl() {
    assert!(
        parse_view_ddl("CREATE TABLE foo (x INT)")
            .unwrap()
            .is_none()
    );
}

#[test]
fn create_materialized_view_parses() {
    let ddl = parse_view_ddl(CREATE_SQL).unwrap().expect("should parse");
    match ddl {
        ViewDdl::Create {
            name,
            or_replace,
            definition,
            sql,
        } => {
            assert_eq!(name, "my_view");
            assert!(!or_replace);
            assert_eq!(definition.view_set_name, "my_view");
            assert_eq!(definition.update_group, 9000);
            assert_eq!(definition.options.min_time_column, "x");
            assert_eq!(definition.options.max_time_column, "x");
            assert_eq!(sql, CREATE_SQL);
        }
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}

#[test]
fn create_or_replace_parses() {
    let sql = CREATE_SQL.replacen(
        "CREATE MATERIALIZED VIEW",
        "CREATE OR REPLACE MATERIALIZED VIEW",
        1,
    );
    let ddl = parse_view_ddl(&sql).unwrap().expect("should parse");
    match ddl {
        ViewDdl::Create { or_replace, .. } => assert!(or_replace),
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}

#[test]
fn drop_materialized_view_parses() {
    let ddl = parse_view_ddl("DROP MATERIALIZED VIEW my_view")
        .unwrap()
        .expect("should parse");
    match ddl {
        ViewDdl::Drop { name, if_exists } => {
            assert_eq!(name, "my_view");
            assert!(!if_exists);
        }
        ViewDdl::Create { .. } => panic!("expected Drop"),
    }
}

#[test]
fn drop_materialized_view_if_exists_parses() {
    let ddl = parse_view_ddl("DROP MATERIALIZED VIEW IF EXISTS my_view")
        .unwrap()
        .expect("should parse");
    match ddl {
        ViewDdl::Drop { name, if_exists } => {
            assert_eq!(name, "my_view");
            assert!(if_exists);
        }
        ViewDdl::Create { .. } => panic!("expected Drop"),
    }
}

#[test]
fn line_comment_prefixed_create_parses() {
    let sql = format!("-- comment from a BI front end\n{CREATE_SQL}");
    let ddl = parse_view_ddl(&sql).unwrap().expect("should parse");
    match ddl {
        ViewDdl::Create { name, .. } => assert_eq!(name, "my_view"),
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}

#[test]
fn block_comment_prefixed_drop_parses() {
    let ddl = parse_view_ddl("/* comment */ DROP MATERIALIZED VIEW my_view")
        .unwrap()
        .expect("should parse");
    match ddl {
        ViewDdl::Drop { name, if_exists } => {
            assert_eq!(name, "my_view");
            assert!(!if_exists);
        }
        ViewDdl::Create { .. } => panic!("expected Drop"),
    }
}

#[test]
fn drop_view_without_materialized_is_not_ddl() {
    assert!(parse_view_ddl("DROP VIEW my_view").unwrap().is_none());
}

#[test]
fn missing_required_option_is_a_named_error() {
    let sql = "CREATE MATERIALIZED VIEW my_view WITH (extract_query = $$SELECT 1$$)";
    let err = parse_view_ddl(sql).unwrap_err();
    assert!(
        format!("{err:#}").contains("count_src_query"),
        "expected the error to name the missing option, got: {err:#}"
    );
}

#[test]
fn unknown_option_is_a_named_error() {
    let sql = CREATE_SQL.replace(
        "update_group = 9000,",
        "update_group = 9000, bogus_option = 'x',",
    );
    let err = parse_view_ddl(&sql).unwrap_err();
    assert!(format!("{err:#}").contains("bogus_option"));
}

#[test]
fn malformed_source_partition_delta_is_a_named_error() {
    let sql = CREATE_SQL.replace(
        "time_column = 'x' \
)",
        "time_column = 'x', source_partition_delta = 'not a delta' \
)",
    );
    let err = parse_view_ddl(&sql).unwrap_err();
    assert!(format!("{err:#}").contains("source_partition_delta"));
}

#[test]
fn empty_merge_sort_order_is_a_named_error() {
    let sql = CREATE_SQL.replace(
        "time_column = 'x' \
)",
        "time_column = 'x', merge_sort_order = ',  ,' \
)",
    );
    let err = parse_view_ddl(&sql).unwrap_err();
    assert!(format!("{err:#}").contains("merge_sort_order"));
}

#[test]
fn invalid_name_charset_is_a_named_error() {
    let sql = CREATE_SQL.replace("my_view", "__my_view");
    let err = parse_view_ddl(&sql).unwrap_err();
    assert!(format!("{err:#}").contains("__"));
}

#[test]
fn unmodelled_clause_on_create_is_rejected() {
    let sql = format!("{CREATE_SQL} (col1)");
    assert!(parse_view_ddl(&sql).is_err());
}

#[test]
fn as_body_form_is_rejected() {
    let sql = "CREATE MATERIALIZED VIEW my_view AS SELECT 1";
    // `MATERIALIZED` consumed, but there is no `WITH (...)` -- `AS` is unmodelled trailing input.
    assert!(parse_view_ddl(sql).is_err());
}

#[test]
fn drop_with_a_second_name_is_rejected() {
    assert!(parse_view_ddl("DROP MATERIALIZED VIEW a, b").is_err());
}

#[test]
fn trailing_second_statement_is_rejected() {
    let sql = format!("{CREATE_SQL}; DROP MATERIALIZED VIEW my_view");
    assert!(parse_view_ddl(&sql).is_err());
}

#[test]
fn trailing_semicolon_alone_is_accepted() {
    let sql = format!("{CREATE_SQL};");
    assert!(parse_view_ddl(&sql).unwrap().is_some());
}

/// Round-trip: each query option parses back to exactly the submitted text.
#[test]
fn query_options_round_trip_verbatim() {
    let extract =
        "SELECT 1 AS x\nFROM t\nWHERE k = 'it''s here' AND time >= '{begin}' AND time < '{end}'";
    let sql = format!(
        "CREATE MATERIALIZED VIEW rt_view WITH (\
           extract_query = $${extract}$$, \
           count_src_query = $$SELECT 0 as count FROM blocks WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'$$, \
           merge_partitions_query = $$SELECT * FROM {{source}}$$, \
           update_group = 1, \
           time_column = 'x'\
         )"
    );
    let ddl = parse_view_ddl(&sql).unwrap().expect("should parse");
    match ddl {
        ViewDdl::Create { definition, .. } => {
            assert_eq!(definition.extract_query, extract);
        }
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}

/// The seeded `log_stats` DDL text must parse back to exactly `log_stats_view_definition()`.
#[test]
fn seeded_log_stats_ddl_round_trips() {
    let sql = log_stats_ddl_text();
    let ddl = parse_view_ddl(&sql)
        .unwrap()
        .expect("log_stats DDL text should parse");
    match ddl {
        ViewDdl::Create {
            name, definition, ..
        } => {
            assert_eq!(name, "log_stats");
            assert_eq!(*definition, log_stats_view_definition());
        }
        ViewDdl::Drop { .. } => panic!("expected Create"),
    }
}
