use crate::properties::dictionary_builder::build_dictionary_from_properties_array;
use datafusion::arrow::array::{Array, DictionaryArray, GenericListArray, Int32Array};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

#[derive(Debug)]
pub struct PropertiesToDict {
    signature: Signature,
}

impl PropertiesToDict {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for PropertiesToDict {
    fn default() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::List(Arc::new(Field::new(
                    "Property",
                    DataType::Struct(Fields::from(vec![
                        Field::new("key", DataType::Utf8, false),
                        Field::new("value", DataType::Utf8, false),
                    ])),
                    false,
                )))],
                Volatility::Immutable,
            ),
        }
    }
}

impl ScalarUDFImpl for PropertiesToDict {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "properties_to_dict"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Dictionary(
            Box::new(DataType::Int32),
            Box::new(DataType::List(Arc::new(Field::new(
                "Property",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, false),
                ])),
                false,
            )))),
        ))
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = args.args;
        if args.len() != 1 {
            return internal_err!("properties_to_dict expects exactly one argument");
        }

        match &args[0] {
            ColumnarValue::Array(array) => {
                let list_array = array
                    .as_any()
                    .downcast_ref::<GenericListArray<i32>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal(
                            "properties_to_dict requires a list array as input".to_string(),
                        )
                    })?;

                let dict_array = build_dictionary_from_properties_array(list_array)?;
                Ok(ColumnarValue::Array(Arc::new(dict_array)))
            }
            ColumnarValue::Scalar(_) => {
                internal_err!("properties_to_dict does not support scalar inputs")
            }
        }
    }
}

// Helper UDF to extract properties array from dictionary for use with standard functions
#[derive(Debug)]
pub struct PropertiesToArray {
    signature: Signature,
}

impl PropertiesToArray {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for PropertiesToArray {
    fn default() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for PropertiesToArray {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "properties_to_array"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        match &arg_types[0] {
            DataType::Dictionary(_, value_type) => Ok(value_type.as_ref().clone()),
            _ => internal_err!("properties_to_array expects a Dictionary input type"),
        }
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = args.args;
        if args.len() != 1 {
            return internal_err!("properties_to_array expects exactly one argument");
        }

        match &args[0] {
            ColumnarValue::Array(array) => {
                // Reconstruct the full array from dictionary
                let dict_array = array
                    .as_any()
                    .downcast_ref::<DictionaryArray<Int32Type>>()
                    .ok_or_else(|| {
                        DataFusionError::Internal(
                            "properties_to_array requires a dictionary array as input".to_string(),
                        )
                    })?;

                // Use Arrow's take function to reconstruct the array
                use datafusion::arrow::compute::take;
                let indices = dict_array.keys();
                let values = dict_array.values();

                let reconstructed = take(values.as_ref(), indices, None)
                    .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))?;

                Ok(ColumnarValue::Array(reconstructed))
            }
            ColumnarValue::Scalar(_) => {
                internal_err!("properties_to_array does not support scalar inputs")
            }
        }
    }
}

// UDF to get length of properties that works with both regular and dictionary arrays
#[derive(Debug)]
pub struct PropertiesLength {
    signature: Signature,
}

impl PropertiesLength {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for PropertiesLength {
    fn default() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl ScalarUDFImpl for PropertiesLength {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "properties_length"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Int32)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = args.args;
        if args.len() != 1 {
            return internal_err!("properties_length expects exactly one argument");
        }

        match &args[0] {
            ColumnarValue::Array(array) => {
                match array.data_type() {
                    DataType::List(_) => {
                        // Handle regular list array
                        let list_array = array
                            .as_any()
                            .downcast_ref::<GenericListArray<i32>>()
                            .ok_or_else(|| {
                                DataFusionError::Internal(
                                    "properties_length: failed to cast to list array".to_string(),
                                )
                            })?;

                        let mut lengths = Vec::with_capacity(list_array.len());
                        for i in 0..list_array.len() {
                            if list_array.is_null(i) {
                                lengths.push(None);
                            } else {
                                let start = list_array.value_offsets()[i] as usize;
                                let end = list_array.value_offsets()[i + 1] as usize;
                                lengths.push(Some((end - start) as i32));
                            }
                        }

                        let length_array = Int32Array::from(lengths);
                        Ok(ColumnarValue::Array(Arc::new(length_array)))
                    }
                    DataType::Dictionary(_, value_type) => {
                        // Handle dictionary array
                        match value_type.as_ref() {
                            DataType::List(_) => {
                                let dict_array = array
                                    .as_any()
                                    .downcast_ref::<DictionaryArray<Int32Type>>()
                                    .ok_or_else(|| {
                                        DataFusionError::Internal(
                                            "properties_length: failed to cast to dictionary array"
                                                .to_string(),
                                        )
                                    })?;

                                let values = dict_array.values();
                                let list_values = values
                                    .as_any()
                                    .downcast_ref::<GenericListArray<i32>>()
                                    .ok_or_else(|| {
                                        DataFusionError::Internal(
                                            "properties_length: dictionary values are not a list array".to_string(),
                                        )
                                    })?;

                                // Pre-compute lengths for each unique value in the dictionary
                                let mut dict_lengths = Vec::with_capacity(list_values.len());
                                for i in 0..list_values.len() {
                                    if list_values.is_null(i) {
                                        dict_lengths.push(None);
                                    } else {
                                        let start = list_values.value_offsets()[i] as usize;
                                        let end = list_values.value_offsets()[i + 1] as usize;
                                        dict_lengths.push(Some((end - start) as i32));
                                    }
                                }

                                // Map dictionary keys to lengths
                                let keys = dict_array.keys();
                                let mut lengths = Vec::with_capacity(keys.len());
                                for i in 0..keys.len() {
                                    if keys.is_null(i) {
                                        lengths.push(None);
                                    } else {
                                        let key_index = keys.value(i) as usize;
                                        if key_index < dict_lengths.len() {
                                            lengths.push(dict_lengths[key_index]);
                                        } else {
                                            return internal_err!(
                                                "Dictionary key index out of bounds"
                                            );
                                        }
                                    }
                                }

                                let length_array = Int32Array::from(lengths);
                                Ok(ColumnarValue::Array(Arc::new(length_array)))
                            }
                            _ => internal_err!(
                                "properties_length: unsupported dictionary value type"
                            ),
                        }
                    }
                    _ => internal_err!(
                        "properties_length: unsupported input type, expected List or Dictionary<Int32, List>"
                    ),
                }
            }
            ColumnarValue::Scalar(_) => {
                internal_err!("properties_length does not support scalar inputs")
            }
        }
    }
}
