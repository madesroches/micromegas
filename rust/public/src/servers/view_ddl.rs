//! `CREATE [OR REPLACE] MATERIALIZED VIEW ... WITH (...)` / `DROP MATERIALIZED VIEW ...` --
//! the DDL front end for DDL-defined eagerly materialized views.
//!
//! All three queries (`extract_query`, `count_src_query`, `merge_partitions_query`) are `WITH`
//! options, so none of them is privileged the way a statement body would be -- that requires
//! parsing the statement directly instead of going through `sqlparser`'s own `parse_create_view`,
//! which unconditionally expects an `AS` body immediately after the view name and so cannot parse
//! a `CREATE MATERIALIZED VIEW ... WITH (...)` with no `AS` body at all. [`parse_view_ddl`]
//! therefore hand-rolls the parse off a single `Parser::try_with_sql`.

use anyhow::{Context, Result};
use datafusion::sql::sqlparser::{
    ast::{Expr as SqlExpr, SqlOption, Value},
    dialect::GenericDialect,
    keywords::Keyword,
    parser::Parser,
    tokenizer::Token,
};
use micromegas_analytics::lakehouse::read_scope::CallerContext;
use micromegas_analytics::lakehouse::view_definition::{
    ViewDefinition, ViewOptions, check_view_set_name_charset, parse_time_delta,
};
use tonic::Status;

/// One parsed `CREATE [OR REPLACE] MATERIALIZED VIEW` / `DROP MATERIALIZED VIEW` statement.
#[derive(Debug, Clone)]
pub enum ViewDdl {
    Create {
        name: String,
        or_replace: bool,
        // Boxed: `ViewDefinition` is far larger than `Drop`'s variant, and clippy's
        // `large_enum_variant` flags the resulting size gap.
        definition: Box<ViewDefinition>,
        /// The verbatim statement text, stored as `lakehouse_view_set_definitions.definition_sql`.
        sql: String,
    },
    Drop {
        name: String,
        if_exists: bool,
    },
}

/// The same admin gate as the nine admin-gated lakehouse UDTFs/UDFs
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

/// `Ok(None)` when `sql` is not view DDL -- the overwhelmingly common case, decided by a cheap
/// keyword peek before any full parse. `Err` when the hand-rolled parse does not reach EOF right
/// after `<name> WITH (...)` (or, on `DROP`, right after the single name): an unmodelled clause,
/// an `AS` body, a second `DROP` name, or a second statement each would otherwise silently do less
/// than the statement says.
///
/// Returns a plain `anyhow::Result` rather than a typed error enum: this crate's convention
/// (`rust/CLAUDE.md`) is to use `anyhow` unless a caller needs to branch on the error kind, and
/// `execute_query`'s only caller here never does -- it always maps the error into a client-facing
/// `invalid_argument` `Status`.
pub fn parse_view_ddl(sql: &str) -> Result<Option<ViewDdl>> {
    let first_word = sql
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '(')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if first_word != "create" && first_word != "drop" {
        return Ok(None);
    }

    let mut parser = Parser::new(&GenericDialect {})
        .try_with_sql(sql)
        .with_context(|| "tokenizing statement")?;

    if parser.parse_keyword(Keyword::CREATE) {
        let or_replace = parser.parse_keywords(&[Keyword::OR, Keyword::REPLACE]);
        if !parser.parse_keyword(Keyword::MATERIALIZED) {
            // CREATE VIEW / CREATE TABLE / ... -- not our DDL, let ctx.sql handle it (or reject
            // it) as usual.
            return Ok(None);
        }
        parser
            .expect_keyword_is(Keyword::VIEW)
            .with_context(|| "expected VIEW after CREATE [OR REPLACE] MATERIALIZED")?;
        let name = parser
            .parse_object_name(false)
            .with_context(|| "expected a view set name")?
            .to_string();
        let options = parser
            .parse_options(Keyword::WITH)
            .with_context(|| "expected WITH (...) options")?;
        require_eof(
            &mut parser,
            "CREATE [OR REPLACE] MATERIALIZED VIEW <name> WITH (...)",
        )?;
        let definition = view_definition_from_options(name.clone(), options)
            .with_context(|| format!("CREATE MATERIALIZED VIEW {name}"))?;
        return Ok(Some(ViewDdl::Create {
            name,
            or_replace,
            definition: Box::new(definition),
            sql: sql.to_string(),
        }));
    }

    if parser.parse_keyword(Keyword::DROP) {
        if !parser.parse_keyword(Keyword::MATERIALIZED) {
            return Ok(None);
        }
        parser
            .expect_keyword_is(Keyword::VIEW)
            .with_context(|| "expected VIEW after DROP MATERIALIZED")?;
        let if_exists = parser.parse_keywords(&[Keyword::IF, Keyword::EXISTS]);
        let name = parser
            .parse_object_name(false)
            .with_context(|| "expected a view set name")?
            .to_string();
        require_eof(&mut parser, "DROP MATERIALIZED VIEW [IF EXISTS] <name>")?;
        return Ok(Some(ViewDdl::Drop { name, if_exists }));
    }

    Ok(None)
}

/// Consumes an optional trailing `;` and requires EOF immediately after -- preserving the
/// single-statement rule `SessionContext::sql`'s `sql_to_statement` enforces for every non-DDL
/// query; intercepting DDL ahead of `ctx.sql` would otherwise bypass that guard and silently
/// execute only the first of several statements.
fn require_eof(parser: &mut Parser, context: &str) -> Result<()> {
    let _ = parser.consume_token(&Token::SemiColon);
    let trailing = parser.peek_token_ref().token.clone();
    if trailing != Token::EOF {
        anyhow::bail!("unexpected trailing input after {context}: {trailing}");
    }
    Ok(())
}

fn value_to_string(expr: &SqlExpr) -> Result<String> {
    match expr {
        SqlExpr::Value(v) => match &v.value {
            Value::SingleQuotedString(s) => Ok(s.clone()),
            Value::DollarQuotedString(d) => Ok(d.value.clone()),
            other => anyhow::bail!("expected a string literal, got {other}"),
        },
        other => anyhow::bail!("expected a string literal, got {other}"),
    }
}

fn value_to_i32(expr: &SqlExpr) -> Result<i32> {
    match expr {
        SqlExpr::Value(v) => match &v.value {
            Value::Number(s, _) => s
                .parse::<i32>()
                .with_context(|| format!("invalid integer {s:?}")),
            Value::SingleQuotedString(s) => s
                .parse::<i32>()
                .with_context(|| format!("invalid integer {s:?}")),
            other => anyhow::bail!("expected an integer literal, got {other}"),
        },
        other => anyhow::bail!("expected an integer literal, got {other}"),
    }
}

/// Assembles the normalized `ViewDefinition` from a `WITH (...)` option list. Option semantics:
/// `extract_query`, `count_src_query`, `merge_partitions_query`, and `update_group` are required;
/// `time_column` is required unless
/// both `min_time_column` and `max_time_column` are given individually; `source_partition_delta`
/// defaults to `'1 day'`; `merge_partition_delta` defaults to `source_partition_delta`;
/// `merge_sort_order` is an optional comma-separated column list. Anything else is a hard error.
fn view_definition_from_options(name: String, options: Vec<SqlOption>) -> Result<ViewDefinition> {
    let mut extract_query = None;
    let mut count_src_query = None;
    let mut merge_partitions_query = None;
    let mut update_group = None;
    let mut time_column = None;
    let mut min_time_column = None;
    let mut max_time_column = None;
    let mut source_partition_delta: Option<String> = None;
    let mut merge_partition_delta: Option<String> = None;
    let mut merge_sort_order: Option<Vec<String>> = None;

    for opt in options {
        let SqlOption::KeyValue { key, value } = opt else {
            anyhow::bail!("unsupported view option (expected `key = value`)");
        };
        let key_name = key.value.to_lowercase();
        match key_name.as_str() {
            "extract_query" => extract_query = Some(value_to_string(&value)?),
            "count_src_query" => count_src_query = Some(value_to_string(&value)?),
            "merge_partitions_query" => merge_partitions_query = Some(value_to_string(&value)?),
            "update_group" => update_group = Some(value_to_i32(&value)?),
            "time_column" => time_column = Some(value_to_string(&value)?),
            "min_time_column" => min_time_column = Some(value_to_string(&value)?),
            "max_time_column" => max_time_column = Some(value_to_string(&value)?),
            "source_partition_delta" => {
                let s = value_to_string(&value)?;
                parse_time_delta(&s).with_context(|| "source_partition_delta")?;
                source_partition_delta = Some(s);
            }
            "merge_partition_delta" => {
                let s = value_to_string(&value)?;
                parse_time_delta(&s).with_context(|| "merge_partition_delta")?;
                merge_partition_delta = Some(s);
            }
            "merge_sort_order" => {
                let s = value_to_string(&value)?;
                let cols: Vec<String> = s
                    .split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect();
                if cols.is_empty() {
                    anyhow::bail!(
                        "merge_sort_order must be a non-empty, comma-separated column list"
                    );
                }
                merge_sort_order = Some(cols);
            }
            other => anyhow::bail!("unknown view option '{other}'"),
        }
    }

    let extract_query = extract_query.context("missing required option 'extract_query'")?;
    let count_src_query = count_src_query.context("missing required option 'count_src_query'")?;
    let merge_partitions_query =
        merge_partitions_query.context("missing required option 'merge_partitions_query'")?;
    let update_group = update_group.context("missing required option 'update_group'")?;

    let (resolved_min, resolved_max) = match (time_column, min_time_column, max_time_column) {
        (Some(t), min, max) => (min.unwrap_or_else(|| t.clone()), max.unwrap_or(t)),
        (None, Some(min), Some(max)) => (min, max),
        _ => anyhow::bail!(
            "missing required option 'time_column' (or both 'min_time_column' and \
             'max_time_column')"
        ),
    };

    let source_partition_delta = source_partition_delta.unwrap_or_else(|| "1 day".to_string());
    let merge_partition_delta =
        merge_partition_delta.unwrap_or_else(|| source_partition_delta.clone());

    check_view_set_name_charset(&name)?;

    Ok(ViewDefinition {
        view_set_name: name,
        extract_query,
        count_src_query,
        merge_partitions_query,
        update_group,
        options: ViewOptions {
            min_time_column: resolved_min,
            max_time_column: resolved_max,
            source_partition_delta,
            merge_partition_delta,
            merge_sort_order,
        },
    })
}
