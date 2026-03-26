/// Unified binary column accessor for Arrow arrays
pub mod binary_column_accessor;
/// Compute histograms from SQL
pub mod histogram;
/// JSONB support
pub mod jsonb;
/// Property UDFs
pub mod properties;

use std::sync::Arc;

use datafusion::logical_expr::ScalarUDF;
use datafusion::prelude::SessionContext;
use histogram::{
    accessors::{make_count_from_histogram_udf, make_sum_from_histogram_udf},
    expand::ExpandHistogramTableFunction,
    histogram_udaf::make_histo_udaf,
    quantile::make_quantile_from_histogram_udf,
    sum_histograms_udaf::sum_histograms_udaf,
    variance::make_variance_from_histogram_udf,
};
use jsonb::{
    array_elements::JsonbArrayElementsTableFunction,
    array_length::make_jsonb_array_length_udf,
    cast::{make_jsonb_as_f64_udf, make_jsonb_as_i64_udf, make_jsonb_as_string_udf},
    each::JsonbEachTableFunction,
    format_json::make_jsonb_format_json_udf,
    get::make_jsonb_get_udf,
    keys::make_jsonb_object_keys_udf,
    parse::make_jsonb_parse_udf,
    path_query::{make_jsonb_path_query_first_udf, make_jsonb_path_query_udf},
};
use properties::{
    properties_udf::{PropertiesLength, PropertiesToArray},
    property_get::PropertyGet,
};

/// Register all extension UDFs on a SessionContext.
pub fn register_extension_udfs(ctx: &SessionContext) {
    ctx.register_udaf(make_histo_udaf());
    ctx.register_udaf(sum_histograms_udaf());
    ctx.register_udf(make_quantile_from_histogram_udf());
    ctx.register_udf(make_variance_from_histogram_udf());
    ctx.register_udf(make_count_from_histogram_udf());
    ctx.register_udf(make_sum_from_histogram_udf());
    ctx.register_udtf(
        "expand_histogram",
        Arc::new(ExpandHistogramTableFunction::new()),
    );

    ctx.register_udf(make_jsonb_parse_udf());
    ctx.register_udf(make_jsonb_format_json_udf());
    ctx.register_udf(make_jsonb_get_udf());
    ctx.register_udf(make_jsonb_as_string_udf());
    ctx.register_udf(make_jsonb_as_f64_udf());
    ctx.register_udf(make_jsonb_as_i64_udf());
    ctx.register_udf(make_jsonb_array_length_udf());
    ctx.register_udf(make_jsonb_object_keys_udf());
    ctx.register_udf(make_jsonb_path_query_first_udf());
    ctx.register_udf(make_jsonb_path_query_udf());
    ctx.register_udtf(
        "jsonb_array_elements",
        Arc::new(JsonbArrayElementsTableFunction::new()),
    );
    ctx.register_udtf("jsonb_each", Arc::new(JsonbEachTableFunction::new()));

    ctx.register_udf(ScalarUDF::from(PropertyGet::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesToArray::new()));
    ctx.register_udf(ScalarUDF::from(PropertiesLength::new()));
}
