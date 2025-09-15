use anyhow::{Result, anyhow};
use datafusion::arrow::array::{Array, ArrayRef, DictionaryArray, RecordBatch, StringArray};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use std::sync::Arc;

pub trait StringColumnAccessor: Send {
    fn value(&self, index: usize) -> &str;

    fn len(&self) -> usize;

    fn is_null(&self, index: usize) -> bool;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct StringArrayAccessor {
    array: Arc<StringArray>,
}

impl StringArrayAccessor {
    fn new(array: Arc<StringArray>) -> Self {
        Self { array }
    }
}

impl StringColumnAccessor for StringArrayAccessor {
    fn value(&self, index: usize) -> &str {
        self.array.value(index)
    }

    fn len(&self) -> usize {
        self.array.len()
    }

    fn is_null(&self, index: usize) -> bool {
        self.array.is_null(index)
    }
}

struct DictionaryStringAccessor {
    array: Arc<DictionaryArray<Int32Type>>,
    values: Arc<StringArray>,
}

impl DictionaryStringAccessor {
    fn new(array: Arc<DictionaryArray<Int32Type>>) -> Result<Self> {
        let values = array
            .values()
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("Dictionary values are not StringArray"))?
            .clone();

        Ok(Self {
            array,
            values: Arc::new(values),
        })
    }
}

impl StringColumnAccessor for DictionaryStringAccessor {
    fn value(&self, index: usize) -> &str {
        let key = self.array.keys().value(index);
        self.values.value(key as usize)
    }

    fn len(&self) -> usize {
        self.array.len()
    }

    fn is_null(&self, index: usize) -> bool {
        self.array.is_null(index)
    }
}

pub fn create_string_accessor(array: &ArrayRef) -> Result<Box<dyn StringColumnAccessor + Send>> {
    match array.data_type() {
        DataType::Utf8 => {
            let string_array = array
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| anyhow!("Failed to downcast to StringArray"))?
                .clone();
            Ok(Box::new(StringArrayAccessor::new(Arc::new(string_array))))
        }
        DataType::Dictionary(key_type, value_type) => {
            if !matches!(value_type.as_ref(), DataType::Utf8) {
                return Err(anyhow!("Dictionary values must be Utf8"));
            }

            match key_type.as_ref() {
                DataType::Int32 => {
                    let dict_array = array
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .ok_or_else(|| anyhow!("Failed to downcast to DictionaryArray<Int32>"))?
                        .clone();
                    Ok(Box::new(DictionaryStringAccessor::new(Arc::new(
                        dict_array,
                    ))?))
                }
                _ => Err(anyhow!("Unsupported dictionary key type: {:?}", key_type)),
            }
        }
        _ => Err(anyhow!(
            "Unsupported array type for string accessor: {:?}",
            array.data_type()
        )),
    }
}

pub fn string_column_by_name(
    batch: &RecordBatch,
    name: &str,
) -> Result<Box<dyn StringColumnAccessor + Send>> {
    let column = batch
        .column_by_name(name)
        .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
    create_string_accessor(column)
}
