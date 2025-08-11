use chrono::{DateTime, Utc};
use datafusion::common::plan_err;
use datafusion::error::DataFusionError;
use datafusion::execution::context::ExecutionProps;
use datafusion::logical_expr::simplify::SimplifyContext;
use datafusion::optimizer::simplify_expressions::ExprSimplifier;
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;

/// Simplifies a DataFusion expression.
pub fn simplify_exp(expr: &Expr) -> datafusion::error::Result<Expr> {
    let execution_props = ExecutionProps::new();
    let info = SimplifyContext::new(&execution_props);
    ExprSimplifier::new(info).simplify(expr.clone())
}

/// Converts a DataFusion expression to a string.
pub fn exp_to_string(expr: &Expr) -> datafusion::error::Result<String> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string)), _metadata) => Ok(string),
        other => {
            plan_err!("can't convert {other:?} to string")
        }
    }
}

/// Converts a DataFusion expression to an i64.
pub fn exp_to_i64(expr: &Expr) -> datafusion::error::Result<i64> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Int64(Some(value)), _metadata) => Ok(value),
        other => {
            plan_err!("can't convert {other:?} to i64")
        }
    }
}

/// Converts a DataFusion expression to a f64.
pub fn exp_to_f64(expr: &Expr) -> datafusion::error::Result<f64> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Float64(Some(value)), _metadata) => Ok(value),
        other => {
            plan_err!("can't convert {other:?} to f64")
        }
    }
}

/// Converts a DataFusion expression to a timestamp.
pub fn exp_to_timestamp(expr: &Expr) -> datafusion::error::Result<DateTime<Utc>> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string)), _metadata) => {
            let ts = chrono::DateTime::parse_from_rfc3339(&string)
                .map_err(|e| DataFusionError::External(e.into()))?;
            Ok(ts.into())
        }
        Expr::Literal(ScalarValue::TimestampNanosecond(Some(ns), timezone), _metadata) => {
            if let Some(tz) = timezone
                && *tz != *"+00:00"
            {
                return plan_err!("Timestamp should be in UTC");
            }
            Ok(DateTime::from_timestamp_nanos(ns))
        }
        other => {
            plan_err!("can't convert {other:?} to timestamp")
        }
    }
}
