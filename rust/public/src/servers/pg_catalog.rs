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

    // register tables gloablly
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

    // and also under their own schema
    let catalog_name = "datafusion";
    ctx.catalog(catalog_name)
        .ok_or_else(|| {
            DataFusionError::Configuration(format!(
                "Catalog not found when registering pg_catalog: {catalog_name}"
            ))
        })?
        .register_schema("pg_catalog", Arc::new(pg_catalog_schema))?;

    ctx.register_udf(create_current_schema_udf().with_aliases(["pg_catalog.current_schema"]));
    ctx.register_udf(create_current_schemas_udf().with_aliases(["pg_catalog.current_schemas"]));
    ctx.register_udf(create_version_udf().with_aliases(["pg_catalog.version"]));
    ctx.register_udf(create_pg_get_userbyid_udf().with_aliases(["pg_catalog.pg_get_userbyid"]));
    ctx.register_udf(
        create_has_table_privilege_2param_udf().with_aliases(["pg_catalog.has_table_privilege"]),
    );

    Ok(())
}
