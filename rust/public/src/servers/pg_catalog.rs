use datafusion::catalog::SchemaProvider;
use datafusion::{error::DataFusionError, prelude::*, sql::TableReference};
use datafusion_postgres::pg_catalog::{
    create_current_schema_udf, create_current_schemas_udf, create_has_table_privilege_2param_udf,
    create_pg_get_userbyid_udf, create_version_udf, PgCatalogSchemaProvider,
};

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

    ctx.register_udf(create_current_schema_udf());
    ctx.register_udf(create_current_schemas_udf());
    ctx.register_udf(create_version_udf());
    ctx.register_udf(create_pg_get_userbyid_udf());
    ctx.register_udf(create_has_table_privilege_2param_udf());

    Ok(())
}
