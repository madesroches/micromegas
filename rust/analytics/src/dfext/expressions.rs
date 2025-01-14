use chrono::{DateTime, Utc};
use datafusion::common::plan_err;
use datafusion::error::DataFusionError;
use datafusion::execution::context::ExecutionProps;
use datafusion::logical_expr::simplify::SimplifyContext;
use datafusion::optimizer::simplify_expressions::ExprSimplifier;
use datafusion::prelude::*;
use datafusion::scalar::ScalarValue;

pub fn simplify_exp(expr: &Expr) -> datafusion::error::Result<Expr> {
    let execution_props = ExecutionProps::new();
    let info = SimplifyContext::new(&execution_props);
    ExprSimplifier::new(info).simplify(expr.clone())
}

pub fn exp_to_string(expr: &Expr) -> datafusion::error::Result<String> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string))) => Ok(string),
        other => {
            plan_err!("can't convert {other:?} to string")
        }
    }
}

pub fn exp_to_timestamp(expr: &Expr) -> datafusion::error::Result<DateTime<Utc>> {
    match simplify_exp(expr)? {
        Expr::Literal(ScalarValue::Utf8(Some(string))) => {
            let ts = chrono::DateTime::parse_from_rfc3339(&string)
                .map_err(|e| DataFusionError::External(e.into()))?;
            Ok(ts.into())
        }
        Expr::Literal(ScalarValue::TimestampNanosecond(Some(ns), timezone)) => {
            if let Some(tz) = timezone {
                if *tz != *"+00:00" {
                    return plan_err!("Timestamp should be in UTC");
                }
            }
            Ok(DateTime::from_timestamp_nanos(ns))
        }
        other => {
            plan_err!("can't convert {other:?} to timestamp")
        }
    }
}
