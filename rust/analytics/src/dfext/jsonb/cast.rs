use datafusion::arrow::array::{
    Array, DictionaryArray, Float64Array, GenericBinaryArray, Int64Array, StringDictionaryBuilder,
};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use jsonb::RawJsonb;
use std::any::Any;
use std::sync::Arc;

/// A scalar UDF that casts a JSONB value to a string.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Dictionary<Int32, Utf8> for memory efficiency.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbAsString {
    signature: Signature,
}

impl JsonbAsString {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbAsString {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_string_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<String>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.as_str() {
        Ok(Some(value)) => Ok(Some(value.to_string())),
        Ok(None) => Ok(None),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbAsString {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_as_string"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::Utf8),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_as_string()");
        }

        match args[0].data_type() {
            DataType::Binary => {
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                let mut dict_builder = StringDictionaryBuilder::<Int32Type>::new();
                for i in 0..binary_array.len() {
                    if binary_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let jsonb_bytes = binary_array.value(i);
                        if let Some(value) = extract_string_from_jsonb(jsonb_bytes)? {
                            dict_builder.append_value(value);
                        } else {
                            dict_builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Binary) =>
            {
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                let binary_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a binary array".into())
                    })?;

                let mut dict_builder = StringDictionaryBuilder::<Int32Type>::new();
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        dict_builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            let jsonb_bytes = binary_values.value(key_index);
                            if let Some(value) = extract_string_from_jsonb(jsonb_bytes)? {
                                dict_builder.append_value(value);
                            } else {
                                dict_builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_as_string"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(dict_builder.finish())))
            }
            _ => internal_err!(
                "jsonb_as_string: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Creates a user-defined function to cast a JSONB value to a string.
pub fn make_jsonb_as_string_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbAsString::new())
}

/// A scalar UDF that casts a JSONB value to a f64.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Float64.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbAsF64 {
    signature: Signature,
}

impl JsonbAsF64 {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbAsF64 {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_f64_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<f64>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.as_f64() {
        Ok(value) => Ok(value),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbAsF64 {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_as_f64"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Float64)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_as_f64()");
        }

        match args[0].data_type() {
            DataType::Binary => {
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                let mut builder = Float64Array::builder(binary_array.len());
                for i in 0..binary_array.len() {
                    if binary_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let jsonb_bytes = binary_array.value(i);
                        if let Some(value) = extract_f64_from_jsonb(jsonb_bytes)? {
                            builder.append_value(value);
                        } else {
                            builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Binary) =>
            {
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                let binary_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a binary array".into())
                    })?;

                let mut builder = Float64Array::builder(dict_array.len());
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            let jsonb_bytes = binary_values.value(key_index);
                            if let Some(value) = extract_f64_from_jsonb(jsonb_bytes)? {
                                builder.append_value(value);
                            } else {
                                builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_as_f64"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
            }
            _ => internal_err!(
                "jsonb_as_f64: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Creates a user-defined function to cast a JSONB value to a f64.
pub fn make_jsonb_as_f64_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbAsF64::new())
}

/// A scalar UDF that casts a JSONB value to an i64.
///
/// Accepts both Binary and Dictionary<Int32, Binary> inputs.
/// Returns Int64.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct JsonbAsI64 {
    signature: Signature,
}

impl JsonbAsI64 {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for JsonbAsI64 {
    fn default() -> Self {
        Self::new()
    }
}

fn extract_i64_from_jsonb(jsonb_bytes: &[u8]) -> Result<Option<i64>> {
    let jsonb = RawJsonb::new(jsonb_bytes);
    match jsonb.as_i64() {
        Ok(value) => Ok(value),
        Err(e) => Err(DataFusionError::External(e.into())),
    }
}

impl ScalarUDFImpl for JsonbAsI64 {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "jsonb_as_i64"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int64)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 1 {
            return internal_err!("wrong number of arguments to jsonb_as_i64()");
        }

        match args[0].data_type() {
            DataType::Binary => {
                let binary_array = args[0]
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting to binary array".into())
                    })?;

                let mut builder = Int64Array::builder(binary_array.len());
                for i in 0..binary_array.len() {
                    if binary_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let jsonb_bytes = binary_array.value(i);
                        if let Some(value) = extract_i64_from_jsonb(jsonb_bytes)? {
                            builder.append_value(value);
                        } else {
                            builder.append_null();
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
            }
            DataType::Dictionary(_, value_type)
                if matches!(value_type.as_ref(), DataType::Binary) =>
            {
                let dict_array = args[0]
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("error casting dictionary array".into())
                    })?;

                let binary_values = dict_array
                    .values()
                    .as_any()
                    .downcast_ref::<GenericBinaryArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal("dictionary values are not a binary array".into())
                    })?;

                let mut builder = Int64Array::builder(dict_array.len());
                for i in 0..dict_array.len() {
                    if dict_array.is_null(i) {
                        builder.append_null();
                    } else {
                        let key_index = dict_array.keys().value(i) as usize;
                        if key_index < binary_values.len() {
                            let jsonb_bytes = binary_values.value(key_index);
                            if let Some(value) = extract_i64_from_jsonb(jsonb_bytes)? {
                                builder.append_value(value);
                            } else {
                                builder.append_null();
                            }
                        } else {
                            return internal_err!(
                                "Dictionary key index out of bounds in jsonb_as_i64"
                            );
                        }
                    }
                }
                Ok(ColumnarValue::Array(Arc::new(builder.finish())))
            }
            _ => internal_err!(
                "jsonb_as_i64: unsupported input type, expected Binary or Dictionary<Int32, Binary>"
            ),
        }
    }
}

/// Creates a user-defined function to cast a JSONB value to an i64.
pub fn make_jsonb_as_i64_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(JsonbAsI64::new())
}
