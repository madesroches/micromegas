use crate::arrow_properties::serialize_property_set_to_jsonb;
use crate::properties::property_set::PropertySet;
use anyhow::Result;
use datafusion::arrow::array::{BinaryArray, DictionaryArray, Int32Array};
use datafusion::arrow::datatypes::Int32Type;
use datafusion::common::DataFusionError;
use std::collections::HashMap;
use std::sync::Arc;

/// JSONB property sets deduplicated by content. Index 0 is the empty set.
///
/// Unlike `PropertySetJsonbDictionaryBuilder`, nothing here depends on the address of a parse
/// arena, so one dictionary can span several blocks.
#[derive(Debug)]
pub struct JsonbDictionary {
    values: Vec<Vec<u8>>,
    index: HashMap<Vec<u8>, i32>,
}

impl JsonbDictionary {
    pub fn new() -> Self {
        let empty = serialize_property_set_to_jsonb(&PropertySet::empty())
            .expect("serializing the empty property set");
        let mut index = HashMap::new();
        index.insert(empty.clone(), 0);
        Self {
            values: vec![empty],
            index,
        }
    }

    /// Returns the index of `jsonb`, adding it if it is new.
    pub fn intern(&mut self, jsonb: &[u8]) -> i32 {
        if let Some(&i) = self.index.get(jsonb) {
            return i;
        }
        let i = self.values.len() as i32;
        self.values.push(jsonb.to_vec());
        self.index.insert(jsonb.to_vec(), i);
        i
    }

    pub fn values(&self) -> &[Vec<u8>] {
        &self.values
    }

    /// Builds the column from one dictionary index per row.
    pub fn finish(self, keys: Vec<i32>) -> Result<DictionaryArray<Int32Type>> {
        let byte_slices: Vec<&[u8]> = self.values.iter().map(|v| v.as_slice()).collect();
        let values = Arc::new(BinaryArray::from_vec(byte_slices));
        DictionaryArray::try_new(Int32Array::from(keys), values)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None).into())
    }
}

impl Default for JsonbDictionary {
    fn default() -> Self {
        Self::new()
    }
}
