//! `deny_queries(match_expr, reason)` -- admin UDTF that validates and inserts a new query-deny-
//! list rule (`tasks/query_deny_list_plan.md` §8). `call_with_args` runs every validation
//! synchronously and fail-loud with `plan_err!`: expression compilation (`compile_match_expr`,
//! against the bare `SessionContext` `QueryDenyList` holds for exactly this, not the caller's own
//! session -- rule compilation must produce the same result on every replica regardless of whose
//! session triggered it), the empty-`reason` check, the caller-identity check
//! (`CallerContext::identity` must be `Some`), and the rule-count check against the current
//! snapshot. Only the DB write and the local snapshot refresh happen in the async body, behind
//! `LogStreamTableProvider`/`TaskLogExecPlan` -- `QueryDenyList::insert` takes the already-compiled
//! expression and cannot itself fail on a bad expression.

use super::query_deny_list::{QueryDenyList, compile_match_expr, max_rules};
use crate::dfext::expressions::exp_to_string;
use crate::dfext::log_stream_table_provider::LogStreamTableProvider;
use crate::dfext::task_log_exec_plan::TaskLogExecPlan;
use datafusion::catalog::TableFunctionArgs;
use datafusion::catalog::TableFunctionImpl;
use datafusion::catalog::TableProvider;
use datafusion::common::plan_err;
use micromegas_tracing::prelude::*;
use std::sync::Arc;

/// A DataFusion `TableFunctionImpl` for `deny_queries(match_expr, reason)`.
#[derive(Debug)]
pub struct DenyQueriesTableFunction {
    query_denials: Arc<QueryDenyList>,
    /// `CallerContext::identity`, captured at registration time (once per request, in
    /// `register_lakehouse_functions`) -- `deny_queries` requires `Some`.
    identity: Option<String>,
}

impl DenyQueriesTableFunction {
    pub fn new(query_denials: Arc<QueryDenyList>, identity: Option<String>) -> Self {
        Self {
            query_denials,
            identity,
        }
    }
}

impl TableFunctionImpl for DenyQueriesTableFunction {
    fn call_with_args(
        &self,
        args: TableFunctionArgs,
    ) -> datafusion::error::Result<Arc<dyn TableProvider>> {
        let exprs = args.exprs();
        let Some(match_expr) = exprs.first().map(exp_to_string).transpose()? else {
            return plan_err!("Missing first argument, expected match_expr: String");
        };
        let Some(reason) = exprs.get(1).map(exp_to_string).transpose()? else {
            return plan_err!("Missing 2nd argument, expected reason: String");
        };
        if reason.trim().is_empty() {
            return plan_err!("deny_queries: reason must not be empty");
        }
        let Some(created_by) = self.identity.clone() else {
            return plan_err!(
                "deny_queries requires an authenticated caller identity, which this session \
                 does not carry"
            );
        };
        let rule_count = self.query_denials.rule_count();
        let cap = max_rules();
        if rule_count >= cap {
            return plan_err!(
                "deny_queries: at the cap of {cap} rules (MICROMEGAS_QUERY_DENY_MAX_RULES); \
                 remove an existing rule with remove_query_denial(rule_id) before adding another"
            );
        }
        // Compiled here, synchronously, against the bare `SessionContext` the list holds for
        // exactly this -- not `args.session()` -- so compilation is independent of which
        // replica's session triggered it (§3/§4 of the plan).
        let compiled = compile_match_expr(self.query_denials.compile_ctx(), &match_expr)?;

        let query_denials = self.query_denials.clone();
        let spawner = move || {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            spawn_with_context(async move {
                match query_denials
                    .insert(&match_expr, compiled, &reason, &created_by)
                    .await
                {
                    Ok(row) => {
                        info!("deny_queries: inserted rule {}", row.rule_id);
                        let _ = tx
                            .send(Ok((chrono::Utc::now(), row.rule_id.to_string())))
                            .await;
                    }
                    Err(e) => {
                        error!("deny_queries: insert failed: {e:?}");
                        let _ = tx.send(Err(format!("{e:?}"))).await;
                    }
                }
            });
            rx
        };

        Ok(Arc::new(LogStreamTableProvider {
            log_stream: Arc::new(TaskLogExecPlan::new(Box::new(spawner))),
        }))
    }
}
