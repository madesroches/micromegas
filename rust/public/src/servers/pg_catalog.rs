use datafusion::catalog::SchemaProvider;
use datafusion::{error::DataFusionError, prelude::*, sql::TableReference};
use datafusion_postgres::pg_catalog::{
    PgCatalogSchemaProvider, create_current_schema_udf, create_current_schemas_udf,
    create_has_table_privilege_2param_udf, create_pg_get_userbyid_udf, create_version_udf,
};
use std::sync::Arc;

/// Sets up the PostgreSQL catalog functions and tables in the DataFusion session context.
pub async fn setup_pg_catalog(ctx: &SessionContext) -> Result<(), Box<DataFusionError>> {
    let pg_catalog_schema = PgCatalogSchemaProvider::new(ctx.state().catalog_list().clone());

    for table_name in pg_catalog_schema.table_names() {
        if let Some(table) = pg_catalog_schema.table(&table_name).await? {
            ctx.register_table(
                TableReference::Bare {
                    table: table_name.into(),
                },
                table,
            )?;
        }
    }

    let catalog_name = "datafusion";
    ctx.catalog(catalog_name)
        .ok_or_else(|| {
            DataFusionError::Configuration(format!(
                "Catalog not found when registering pg_catalog: {catalog_name}"
            ))
        })?
        .register_schema("pg_catalog", Arc::new(pg_catalog_schema))?;

    //todo: use with_aliases to avoid "Invalid function 'pg_catalog.current_schemas'"
    ctx.register_udf(create_current_schema_udf());
    ctx.register_udf(create_current_schemas_udf());
    ctx.register_udf(create_version_udf());
    ctx.register_udf(create_pg_get_userbyid_udf());
    ctx.register_udf(create_has_table_privilege_2param_udf());

    Ok(())
}
