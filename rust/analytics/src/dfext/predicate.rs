use datafusion::{
    arrow::datatypes::SchemaRef,
    catalog::Session,
    common::DFSchema,
    logical_expr::utils::conjunction,
    physical_plan::{expressions, PhysicalExpr},
    prelude::*,
};
use std::sync::Arc;

// from datafusion/datafusion-examples/examples/advanced_parquet_index.rs
pub fn filters_to_predicate(
    schema: SchemaRef,
    state: &dyn Session,
    filters: &[Expr],
) -> datafusion::error::Result<Arc<dyn PhysicalExpr>> {
    let df_schema = DFSchema::try_from(schema)?;
    let predicate = conjunction(filters.to_vec());
    let predicate = predicate
        .map(|predicate| state.create_physical_expr(predicate, &df_schema))
        .transpose()?
        // if there are no filters, use a literal true to have a predicate
        // that always evaluates to true we can pass to the index
        .unwrap_or_else(|| expressions::lit(true));

    Ok(predicate)
}
