use anyhow::{Result, anyhow};
use datafusion::arrow::array::{Array, ArrayRef, BinaryArray, DictionaryArray, RecordBatch};
use datafusion::arrow::datatypes::{DataType, Int32Type};
use std::sync::Arc;

pub trait BinaryColumnAccessor: Send {
    fn value(&self, index: usize) -> &[u8];

    fn len(&self) -> usize;

    fn is_null(&self, index: usize) -> bool;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct BinaryArrayAccessor {
    array: Arc<BinaryArray>,
}

impl BinaryArrayAccessor {
    fn new(array: Arc<BinaryArray>) -> Self {
        Self { array }
    }
}

impl BinaryColumnAccessor for BinaryArrayAccessor {
    fn value(&self, index: usize) -> &[u8] {
        self.array.value(index)
    }

    fn len(&self) -> usize {
        self.array.len()
    }

    fn is_null(&self, index: usize) -> bool {
        self.array.is_null(index)
    }
}

struct DictionaryBinaryAccessor {
    array: Arc<DictionaryArray<Int32Type>>,
    values: Arc<BinaryArray>,
}

impl DictionaryBinaryAccessor {
    fn new(array: Arc<DictionaryArray<Int32Type>>) -> Result<Self> {
        let values = array
            .values()
            .as_any()
            .downcast_ref::<BinaryArray>()
            .ok_or_else(|| anyhow!("Dictionary values are not BinaryArray"))?
            .clone();

        Ok(Self {
            array,
            values: Arc::new(values),
        })
    }
}

impl BinaryColumnAccessor for DictionaryBinaryAccessor {
    fn value(&self, index: usize) -> &[u8] {
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

pub fn create_binary_accessor(array: &ArrayRef) -> Result<Box<dyn BinaryColumnAccessor + Send>> {
    match array.data_type() {
        DataType::Binary => {
            let binary_array = array
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or_else(|| anyhow!("Failed to downcast to BinaryArray"))?
                .clone();
            Ok(Box::new(BinaryArrayAccessor::new(Arc::new(binary_array))))
        }
        DataType::Dictionary(key_type, value_type) => {
            if !matches!(value_type.as_ref(), DataType::Binary) {
                return Err(anyhow!("Dictionary values must be Binary"));
            }

            match key_type.as_ref() {
                DataType::Int32 => {
                    let dict_array = array
                        .as_any()
                        .downcast_ref::<DictionaryArray<Int32Type>>()
                        .ok_or_else(|| anyhow!("Failed to downcast to DictionaryArray<Int32>"))?
                        .clone();
                    Ok(Box::new(DictionaryBinaryAccessor::new(Arc::new(
                        dict_array,
                    ))?))
                }
                _ => Err(anyhow!("Unsupported dictionary key type: {:?}", key_type)),
            }
        }
        _ => Err(anyhow!(
            "Unsupported array type for binary accessor: {:?}",
            array.data_type()
        )),
    }
}

pub fn binary_column_by_name(
    batch: &RecordBatch,
    name: &str,
) -> Result<Box<dyn BinaryColumnAccessor + Send>> {
    let column = batch
        .column_by_name(name)
        .ok_or_else(|| anyhow!("Column '{}' not found", name))?;
    create_binary_accessor(column)
}
