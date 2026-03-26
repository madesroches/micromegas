# Allow `expand_histogram` to Accept Expression Arguments Plan

GitHub Issue: #983

## Overview

`expand_histogram` in `rust/datafusion-extensions/src/histogram/expand.rs` rejects non-literal/non-subquery expression arguments and doesn't handle Dictionary-encoded scalar values. This is the same limitation that was fixed for `jsonb_each` in #982 and already handled in `jsonb_array_elements`. Apply the same two-part pattern: wrap arbitrary expressions via `LogicalPlanBuilder`, and unwrap `ScalarValue::Dictionary` in `scalar_to_batch()`.

## Current State

`ExpandHistogramTableFunction::call()` in `expand.rs:52-72` matches only `Expr::Literal` and `Expr::ScalarSubquery`:

```rust
let source = match &args[0] {
    Expr::Literal(scalar, _metadata) => HistogramSource::Literal(scalar.clone()),
    Expr::ScalarSubquery(subquery) => HistogramSource::Subquery(subquery.subquery.clone()),
    other => {
        return Err(DataFusionError::Plan(format!(
            "expand_histogram argument must be a histogram literal or subquery, got: {other:?}"
        )));
    }
};
```

`scalar_to_batch()` at lines 127-136 only handles `ScalarValue::Struct`:

```rust
fn scalar_to_batch(scalar: &ScalarValue) -> Result<RecordBatch, DataFusionError> {
    if let ScalarValue::Struct(struct_array) = scalar {
        extract_histogram_from_struct(struct_array)
    } else {
        Err(DataFusionError::Plan(format!(
            "expand_histogram argument must be a struct (histogram), got: {:?}",
            scalar.data_type()
        )))
    }
}
```

DataFusion's `ExprSimplifier` can constant-fold expressions into `ScalarValue::Dictionary(_, Struct)` before `call()` receives them, causing both a `Literal` path rejection and an inability to pass expressions like `make_histogram(...)` directly.

The fix pattern is established by `jsonb_each` (#982) and `jsonb_array_elements` (#986).

## Design

Two changes, identical in spirit to the `jsonb_each` fix:

1. **`call()` catch-all**: Replace the error arm with `LogicalPlanBuilder::empty(true).project(vec![expr]).build()`, storing the result as `HistogramSource::Subquery`. This reuses the existing subquery execution path in `scan()`.

2. **`scalar_to_batch()` Dictionary handling**: Add a `ScalarValue::Dictionary(_, inner) => scalar_to_batch(inner.as_ref())` arm to recursively unwrap dictionary-encoded struct scalars.

## Implementation Steps

### 1. Update `scalar_to_batch()` in `rust/datafusion-extensions/src/histogram/expand.rs`

Add a match arm for `ScalarValue::Dictionary`:

```rust
fn scalar_to_batch(scalar: &ScalarValue) -> Result<RecordBatch, DataFusionError> {
    match scalar {
        ScalarValue::Struct(struct_array) => extract_histogram_from_struct(struct_array),
        ScalarValue::Dictionary(_, inner) => scalar_to_batch(inner.as_ref()),
        _ => Err(DataFusionError::Plan(format!(
            "expand_histogram argument must be a struct (histogram), got: {:?}",
            scalar.data_type()
        ))),
    }
}
```

### 2. Update `call()` in `rust/datafusion-extensions/src/histogram/expand.rs`

Replace the catch-all error arm:

```rust
other => {
    let plan = LogicalPlanBuilder::empty(true)
        .project(vec![other.clone()])?
        .build()?;
    HistogramSource::Subquery(Arc::new(plan))
}
```

Add the import:

```rust
use datafusion::logical_expr::{LogicalPlan, LogicalPlanBuilder};
```

(Replace the existing `use datafusion::logical_expr::LogicalPlan;` import.)

### 3. Add tests in `rust/datafusion-extensions/tests/expand_histogram_tests.rs`

New test file with:

- `test_call_accepts_cast_expression` — unit test verifying `call()` returns `Ok` for a non-literal expression (e.g., `Expr::Cast`)
- SQL integration test using `expand_histogram(make_histogram(...))` with direct expression argument (if feasible with existing test data setup)

## Files to Modify

| File | Change |
|------|--------|
| `rust/datafusion-extensions/src/histogram/expand.rs` | Handle Dictionary scalars in `scalar_to_batch()`; replace error arm with `LogicalPlanBuilder` wrapping; update import |
| `rust/datafusion-extensions/tests/expand_histogram_tests.rs` | **New** — unit test for expression wrapping path |

## Trade-offs

**Why the same approach as `jsonb_each`?**
The three UDTFs (`expand_histogram`, `jsonb_each`, `jsonb_array_elements`) share the same `Literal`/`Subquery` source pattern. Using the identical fix ensures consistency and reduces cognitive overhead. A shared abstraction could be extracted, but that's unnecessary complexity for a two-line change per function.

**Why not also update `ExpandHistogramTableProvider::from_scalar()`?**
`from_scalar()` at line 146 only checks for `ScalarValue::Struct`. It could be updated to accept Dictionary-wrapped structs too, but it's a convenience constructor only used for testing. Not worth the scope creep — if needed later, it's trivial.

## Testing Strategy

1. `cargo build` — verify compilation
2. `cargo clippy --workspace -- -D warnings` — no warnings
3. `cargo test -p micromegas-datafusion-extensions` — run existing tests to ensure no regressions
4. New test: `call()` with `Expr::Cast` returns `Ok` (exercises the wrapping path, same as the `jsonb_each` test pattern)

## Open Questions

None — the fix is a direct application of the pattern established in #982.
