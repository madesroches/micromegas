use datafusion::arrow::array::{
    Array, AsArray, DictionaryArray, GenericListArray, Int32Array, ListBuilder, StringBuilder,
    StructArray, StructBuilder,
};
use datafusion::arrow::datatypes::{DataType, Field, Fields, Int32Type};
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::collections::HashMap;
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

                let dict_array = build_dictionary_from_properties(list_array)?;
                Ok(ColumnarValue::Array(Arc::new(dict_array)))
            }
            ColumnarValue::Scalar(_) => {
                internal_err!("properties_to_dict does not support scalar inputs")
            }
        }
    }
}

struct PropertiesDictionaryBuilder {
    map: HashMap<Vec<(String, String)>, usize>,
    values_builder: ListBuilder<StructBuilder>,
    keys: Vec<Option<i32>>,
}

impl PropertiesDictionaryBuilder {
    fn new(capacity: usize) -> Self {
        let prop_struct_fields = vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ];
        let prop_field = Arc::new(Field::new(
            "Property",
            DataType::Struct(Fields::from(prop_struct_fields.clone())),
            false,
        ));
        let values_builder =
            ListBuilder::new(StructBuilder::from_fields(prop_struct_fields, capacity))
                .with_field(prop_field);

        Self {
            map: HashMap::new(),
            values_builder,
            keys: Vec::with_capacity(capacity),
        }
    }

    fn append_property_list(&mut self, struct_array: &StructArray) -> Result<()> {
        let prop_vec = extract_properties_as_vec(struct_array)?;

        match self.map.get(&prop_vec) {
            Some(&index) => {
                self.keys.push(Some(index as i32));
            }
            None => {
                let new_index = self.map.len();
                self.add_to_values(&prop_vec)?;
                self.map.insert(prop_vec, new_index);
                self.keys.push(Some(new_index as i32));
            }
        }
        Ok(())
    }

    fn append_null(&mut self) {
        self.keys.push(None);
    }

    fn add_to_values(&mut self, properties: &[(String, String)]) -> Result<()> {
        let struct_builder = self.values_builder.values();
        for (key, value) in properties {
            struct_builder
                .field_builder::<StringBuilder>(0)
                .ok_or_else(|| DataFusionError::Internal("Failed to get key builder".to_string()))?
                .append_value(key);
            struct_builder
                .field_builder::<StringBuilder>(1)
                .ok_or_else(|| {
                    DataFusionError::Internal("Failed to get value builder".to_string())
                })?
                .append_value(value);
            struct_builder.append(true);
        }
        self.values_builder.append(true);
        Ok(())
    }

    fn finish(mut self) -> Result<DictionaryArray<Int32Type>> {
        let keys = Int32Array::from(self.keys);
        let values = Arc::new(self.values_builder.finish());
        DictionaryArray::try_new(keys, values)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))
    }
}

fn extract_properties_as_vec(struct_array: &StructArray) -> Result<Vec<(String, String)>> {
    let mut properties = Vec::with_capacity(struct_array.len());
    let key_array = struct_array.column(0).as_string::<i32>();
    let value_array = struct_array.column(1).as_string::<i32>();
    for i in 0..struct_array.len() {
        if struct_array.is_valid(i) {
            let key = key_array.value(i).to_string();
            let value = value_array.value(i).to_string();
            properties.push((key, value));
        }
    }

    Ok(properties)
}

pub fn build_dictionary_from_properties(
    list_array: &GenericListArray<i32>,
) -> Result<DictionaryArray<Int32Type>> {
    let mut builder = PropertiesDictionaryBuilder::new(list_array.len());
    for i in 0..list_array.len() {
        if list_array.is_null(i) {
            builder.append_null();
        } else {
            let start = list_array.value_offsets()[i] as usize;
            let end = list_array.value_offsets()[i + 1] as usize;
            let sliced_values = list_array.values().slice(start, end - start);
            let struct_array = sliced_values.as_struct();
            builder.append_property_list(struct_array)?;
        }
    }

    builder.finish()
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
