# Allow `jsonb_each` to Accept Expression Arguments Plan

GitHub Issue: #978

## Overview

`jsonb_each` currently only accepts `Expr::Literal` and `Expr::ScalarSubquery` arguments in its `TableFunctionImpl::call()` method. Passing any other expression type (e.g., `jsonb_parse('...')` or `jsonb_path_query(...)`) fails with an error. This change makes `jsonb_each` accept arbitrary expressions by wrapping them in a logical plan that can be executed during `scan()`.

## Current State

`JsonbEachTableFunction::call()` in `rust/datafusion-extensions/src/jsonb/each.rs:52-72` matches only two expression variants:

```rust
let source = match &args[0] {
    Expr::Literal(scalar, _metadata) => JsonbSource::Literal(scalar.clone()),
    Expr::ScalarSubquery(subquery) => JsonbSource::Subquery(subquery.subquery.clone()),
    other => {
        return Err(DataFusionError::Plan(format!(
            "jsonb_each argument must be a JSONB literal or subquery, got: {other:?}"
        )));
    }
};
```

The `JsonbSource` enum has two variants: `Literal(ScalarValue)` and `Subquery(Arc<LogicalPlan>)`.

The `Subquery` path in `scan()` (lines 228-252) already handles executing a `LogicalPlan`, collecting batches, and extracting JSONB entries from the resulting column. This path handles both `Binary` and `Dictionary<Int32, Binary>` columns via `extract_all_jsonb_bytes_from_column()`.

However, the `Literal` path has a gap: `scalar_to_entries()` (lines 127-136) only matches `ScalarValue::Binary`. DataFusion 52.4's `ExprSimplifier` runs on all UDTF arguments in `get_table_function_source()` *before* `call()` receives them — it constant-folds expressions like `jsonb_parse('{"a":1}')` into `ScalarValue::Dictionary(Int32, Binary)` literals. These hit the `Literal` arm but are rejected by `scalar_to_entries()`.

`expand_histogram` in `rust/datafusion-extensions/src/histogram/expand.rs` has the same limitation but is out of scope for this issue.

## Design

Replace the catch-all error arm in `call()` with logic that wraps the expression in a `LogicalPlan`. Use DataFusion's `LogicalPlanBuilder::empty(true).project(vec![expr]).build()` to create a plan that:

1. Produces a single row (`EmptyRelation` with `produce_one_row: true`)
2. Evaluates the expression as a projection over that row

This produces a `LogicalPlan` that fits the existing `JsonbSource::Subquery` execution path — no changes needed in `scan()`.

```
                    jsonb_each(jsonb_parse('{"a":1}'))
                                  │
                    call() receives Expr::ScalarFunction
                                  │
                    Wrap in: LogicalPlanBuilder::empty(true)
                                .project(vec![expr])
                                .build()
                                  │
                    Store as JsonbSource::Subquery(plan)
                                  │
                    scan() executes plan → RecordBatch with 1 Binary column
                                  │
                    Existing extraction logic handles the rest
```

### Scope Limitation

This approach works for expressions that don't reference columns from outer tables (e.g., `jsonb_parse('...')`). The lateral join pattern (`FROM events, jsonb_each(jsonb_path_query(events.col, ...))`) requires DataFusion-level lateral join support, which is a separate concern. The `EmptyRelation` plan will fail at physical planning time if the expression references unresolved columns, producing a clear error message.

## Implementation Steps

### 1. ~~Add failing tests to `rust/datafusion-extensions/tests/jsonb_each_tests.rs`~~ DONE

Added three tests that demonstrate the current limitation. All fail as expected:

- `test_jsonb_each_with_jsonb_parse_expression` — SQL: `jsonb_each(jsonb_parse('{"a": 1, "b": 2}'))`, fails with `Dictionary(Int32, Binary)` error (confirms simplifier folds to Dictionary literal)
- `test_jsonb_each_with_jsonb_parse_composability` — SQL: same with `jsonb_as_string(value)`, same failure
- `test_call_accepts_cast_expression` — unit test: `call()` with `Expr::Cast`, fails with catch-all error arm

### 2. ~~Update `scalar_to_entries()` to handle Dictionary scalars in `rust/datafusion-extensions/src/jsonb/each.rs`~~ DONE

DataFusion's `ExprSimplifier` constant-folds expressions like `jsonb_parse('...')` into `ScalarValue::Dictionary(Int32, Binary)` before `call()` sees them. These arrive as `Expr::Literal` but `scalar_to_entries()` only handles `ScalarValue::Binary`. Add a match arm to unwrap dictionary-encoded binary values:

```rust
fn scalar_to_entries(scalar: &ScalarValue) -> Result<Vec<(String, Vec<u8>)>, DataFusionError> {
    match scalar {
        ScalarValue::Binary(Some(bytes)) => extract_entries_from_jsonb(bytes),
        ScalarValue::Binary(None) => Ok(vec![]),
        ScalarValue::Dictionary(_, inner) => scalar_to_entries(inner.as_ref()),
        _ => Err(DataFusionError::Plan(format!(
            "jsonb_each argument must be Binary (JSONB), got: {:?}",
            scalar.data_type()
        ))),
    }
}
```

### 3. ~~Modify `call()` in `rust/datafusion-extensions/src/jsonb/each.rs`~~ DONE

Replace the catch-all error arm:

```rust
other => {
    return Err(DataFusionError::Plan(format!(
        "jsonb_each argument must be a JSONB literal or subquery, got: {other:?}"
    )));
}
```

With:

```rust
other => {
    let plan = LogicalPlanBuilder::empty(true)
        .project(vec![other.clone()])?
        .build()?;
    JsonbSource::Subquery(Arc::new(plan))
}
```

Add the import:

```rust
use datafusion::logical_expr::LogicalPlanBuilder;
```

## Files to Modify

| File | Change |
|------|--------|
| `rust/datafusion-extensions/src/jsonb/each.rs` | Handle Dictionary scalars in `scalar_to_entries()`; replace error arm with `LogicalPlanBuilder` wrapping; add import |
| `rust/datafusion-extensions/tests/jsonb_each_tests.rs` | Add SQL integration tests for expression args + unit test for wrapping path |

## Trade-offs

**Why wrap in `LogicalPlanBuilder` instead of adding a new `JsonbSource::Expression` variant?**
Reusing the existing `Subquery` path means zero changes to `scan()`. The `EmptyRelation` + projection produces exactly the same execution shape as a scalar subquery. Adding a new variant would duplicate the execution logic or require a shared helper — unnecessary complexity.

**Why not also fix `expand_histogram`?**
Same pattern, same fix, but separate concern. Can be done as a follow-up if needed.

## Testing Strategy

1. `cargo build` — verify compilation with new import
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test -p micromegas-datafusion-extensions` — run existing + new tests
4. New test cases:
   - `jsonb_each(jsonb_parse('{"a": 1, "b": 2}'))` returns 2 rows with correct keys (exercises Dictionary scalar in `Literal` path)
   - `jsonb_each(jsonb_parse('{}'))` returns 0 rows
   - Composability: `SELECT key, jsonb_as_string(value) FROM jsonb_each(jsonb_parse(...))`
   - Unit test: `JsonbEachTableFunction::call()` with synthetic `Expr::ScalarFunction` returns `Ok` (exercises wrapping path)

## Open Questions

None — the fix is a small, well-scoped change that reuses existing infrastructure.
