//! `remove_query_denial(rule_id)` -- admin scalar UDF that deletes a query-deny-list rule. The
//! audit log is the durable record of what was denied and what it rejected, so the row itself
//! does not need to survive its removal (hard delete, no `removed_at`/`removed_by` trail).

use async_trait::async_trait;
use datafusion::{
    arrow::{
        array::{Array, StringArray, StringBuilder},
        datatypes::DataType,
    },
    common::internal_err,
    error::DataFusionError,
    logical_expr::{
        ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
        async_udf::AsyncScalarUDFImpl,
    },
};
use std::sync::Arc;
use uuid::Uuid;

use super::query_deny_list::QueryDenyList;

/// A scalar UDF that removes a single query-deny-list rule by id.
#[derive(Debug)]
pub struct RemoveQueryDenial {
    signature: Signature,
    query_denials: Arc<QueryDenyList>,
}

impl PartialEq for RemoveQueryDenial {
    fn eq(&self, other: &Self) -> bool {
        self.signature == other.signature
    }
}

impl Eq for RemoveQueryDenial {}

impl std::hash::Hash for RemoveQueryDenial {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.signature.hash(state);
    }
}

impl RemoveQueryDenial {
    pub fn new(query_denials: Arc<QueryDenyList>) -> Self {
        Self {
            signature: Signature::exact(vec![DataType::Utf8], Volatility::Volatile),
            query_denials,
        }
    }
}

impl ScalarUDFImpl for RemoveQueryDenial {
    fn name(&self) -> &str {
        "remove_query_denial"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> datafusion::error::Result<DataType> {
        Ok(DataType::Utf8)
    }

    fn invoke_with_args(
        &self,
        _args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        Err(DataFusionError::NotImplemented(
            "remove_query_denial can only be called from async contexts".into(),
        ))
    }
}

#[async_trait]
impl AsyncScalarUDFImpl for RemoveQueryDenial {
    async fn invoke_async_with_args(
        &self,
        args: ScalarFunctionArgs,
    ) -> datafusion::error::Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("remove_query_denial expects exactly 1 argument: rule_id");
        }
        let rule_ids: &StringArray = args[0].as_any().downcast_ref::<_>().ok_or_else(|| {
            DataFusionError::Internal("error casting rule_id argument as StringArray".into())
        })?;

        let mut builder = StringBuilder::with_capacity(rule_ids.len(), 64);
        for index in 0..rule_ids.len() {
            if rule_ids.is_null(index) {
                builder.append_value("ERROR: rule_id cannot be null");
                continue;
            }
            let raw = rule_ids.value(index);
            let message = match Uuid::parse_str(raw) {
                Err(_) => format!("ERROR: '{raw}' is not a valid rule id"),
                Ok(rule_id) => match self.query_denials.delete(rule_id).await {
                    Ok(true) => format!("SUCCESS: removed rule {rule_id}"),
                    Ok(false) => format!("ERROR: no such rule: {rule_id}"),
                    Err(e) => format!("ERROR: {e:?}"),
                },
            };
            builder.append_value(message);
        }

        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates a user-defined function to remove a single query-deny-list rule by id.
///
/// # Usage
/// ```sql
/// SELECT remove_query_denial('9f2c41ab-73de-0155-...') as result;
/// ```
pub fn make_remove_query_denial_udf(
    query_denials: Arc<QueryDenyList>,
) -> datafusion::logical_expr::async_udf::AsyncScalarUDF {
    datafusion::logical_expr::async_udf::AsyncScalarUDF::new(Arc::new(RemoveQueryDenial::new(
        query_denials,
    )))
}
