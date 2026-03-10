use crate::binary_column_accessor::create_binary_accessor;
use datafusion::arrow::array::{Array, BinaryDictionaryBuilder, StringArray};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use jsonb::jsonpath::parse_json_path;
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy)]
enum PathQueryMode {
    First,
    All,
}

fn eval_jsonb_path_query(
    func_name: &str,
    args: ScalarFunctionArgs,
    mode: PathQueryMode,
) -> Result<ColumnarValue> {
    let args = ColumnarValue::values_to_arrays(&args.args)?;
    if args.len() != 2 {
        return internal_err!("wrong number of arguments to {func_name}");
    }

    let accessor = create_binary_accessor(&args[0]).map_err(|e| {
        DataFusionError::Execution(format!(
            "Invalid input type for {func_name}: {e}. Expected Binary or Dictionary<Int32, Binary>"
        ))
    })?;

    let paths = args[1]
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| {
            DataFusionError::Execution(format!("second argument to {func_name} must be a string"))
        })?;

    let mut builder = BinaryDictionaryBuilder::<Int32Type>::new();
    let mut path_cache: HashMap<&str, _> = HashMap::new();

    for i in 0..accessor.len() {
        if accessor.is_null(i) || paths.is_null(i) {
            builder.append_null();
        } else {
            let path_str = paths.value(i);
            if !path_cache.contains_key(path_str) {
                let parsed = parse_json_path(path_str.as_bytes()).map_err(|e| {
                    DataFusionError::Execution(format!(
                        "{func_name}: invalid JSONPath '{path_str}': {e}"
                    ))
                })?;
                path_cache.insert(path_str, parsed);
            }
            let json_path = path_cache.get(path_str).expect("just inserted");
            let raw = RawJsonb::new(accessor.value(i));
            match mode {
                PathQueryMode::First => match raw.select_first_by_path(json_path) {
                    Ok(Some(value)) => builder.append_value(value.as_ref()),
                    Ok(None) => builder.append_null(),
                    Err(e) => return Err(DataFusionError::External(e.into())),
                },
                PathQueryMode::All => match raw.select_array_by_path(json_path) {
                    Ok(value) => builder.append_value(value.as_ref()),
                    Err(e) => return Err(DataFusionError::External(e.into())),
                },
            }
        }
    }

    Ok(ColumnarValue::Array(Arc::new(builder.finish())))
}

/// A scalar UDF that returns the first match of a JSONPath expression on a JSONB value.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs for the JSONB argument.
/// The path argument is Utf8. Returns Dictionary<Int32, Binary> for memory efficiency,
/// or NULL if no match is found.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbPathQueryFirst {
    signature: Signature,
}

impl JsonbPathQueryFirst {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

impl Default for JsonbPathQueryFirst {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbPathQueryFirst {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_path_query_first"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Binary),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        eval_jsonb_path_query("jsonb_path_query_first", args, PathQueryMode::First)
    }
}

/// Creates a user-defined function to extract the first JSONPath match from a JSONB value.
pub fn make_jsonb_path_query_first_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbPathQueryFirst::new())
}

/// A scalar UDF that returns all matches of a JSONPath expression on a JSONB value as a JSONB array.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs for the JSONB argument.
/// The path argument is Utf8. Returns Dictionary<Int32, Binary> containing a JSONB array
/// of all matched values.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbPathQuery {
    signature: Signature,
}

impl JsonbPathQuery {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}

impl Default for JsonbPathQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for JsonbPathQuery {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_path_query"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Binary),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        eval_jsonb_path_query("jsonb_path_query", args, PathQueryMode::All)
    }
}

/// Creates a user-defined function to extract all JSONPath matches from a JSONB value as a JSONB array.
pub fn make_jsonb_path_query_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbPathQuery::new())
}
