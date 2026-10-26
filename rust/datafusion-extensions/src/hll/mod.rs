//! HyperLogLog sketches serialized in the Apache DataSketches format.
pub mod cardinality;
pub mod make_hll_udaf;
pub mod merge_hll_udaf;

use datafusion::arrow::array::{Array, ArrayRef, BinaryArray};
use datafusion::arrow::datatypes::DataType;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::Accumulator;
use datafusion::scalar::ScalarValue;
use datasketches::hll::{HllSketch, HllType, HllUnion};

/// Precision shared by every sketch built here; 2^14 registers gives ~0.8% standard error.
pub const LG_K: u8 = 14;

/// Serializes as Hll4, the most compact target type.
pub fn serialize_sketch(union: &HllUnion) -> Vec<u8> {
    union.to_sketch(HllType::Hll4).serialize()
}

pub fn deserialize_sketch(func_name: &str, bytes: &[u8]) -> Result<HllSketch> {
    HllSketch::deserialize(bytes).map_err(|e| {
        DataFusionError::Execution(format!("{func_name}: invalid HyperLogLog sketch: {e}"))
    })
}

pub fn as_binary_array<'a>(func_name: &str, array: &'a ArrayRef) -> Result<&'a BinaryArray> {
    array.as_any().downcast_ref::<BinaryArray>().ok_or_else(|| {
        DataFusionError::Execution(format!(
            "{func_name}: expected Binary, got {}",
            array.data_type()
        ))
    })
}

pub fn binary_type() -> DataType {
    DataType::Binary
}

/// Adds the rows of an input column to the union.
pub type AddRowsFn = fn(&mut HllUnion, &ArrayRef) -> Result<()>;

/// Accumulator shared by `make_hll` and `merge_hll`; they differ only in how input rows reach the
/// union. Intermediate state is always a serialized sketch.
#[derive(Debug)]
pub struct HllAccumulator {
    func_name: &'static str,
    union: HllUnion,
    add_rows: AddRowsFn,
}

impl HllAccumulator {
    pub fn new(func_name: &'static str, add_rows: AddRowsFn) -> Result<Self> {
        let union = HllUnion::new(LG_K)
            .map_err(|e| DataFusionError::Internal(format!("{func_name}: {e}")))?;
        Ok(Self {
            func_name,
            union,
            add_rows,
        })
    }

    fn to_scalar(&self) -> ScalarValue {
        if self.union.is_empty() {
            ScalarValue::Binary(None)
        } else {
            ScalarValue::Binary(Some(serialize_sketch(&self.union)))
        }
    }
}

impl Accumulator for HllAccumulator {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        match values {
            [array] => (self.add_rows)(&mut self.union, array),
            _ => Err(DataFusionError::Execution(format!(
                "{}: expected exactly one argument",
                self.func_name
            ))),
        }
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        match states {
            [array] => merge_sketches(self.func_name, &mut self.union, array),
            _ => Err(DataFusionError::Internal(format!(
                "{}: expected exactly one state column",
                self.func_name
            ))),
        }
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![self.to_scalar()])
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        Ok(self.to_scalar())
    }

    fn size(&self) -> usize {
        size_of::<Self>() - size_of::<HllUnion>() + self.union.estimated_size()
    }
}

/// Unions every non-null serialized sketch in `array` into `union`.
pub fn merge_sketches(func_name: &str, union: &mut HllUnion, array: &ArrayRef) -> Result<()> {
    let binary = as_binary_array(func_name, array)?;
    for i in 0..binary.len() {
        if binary.is_null(i) {
            continue;
        }
        let sketch = deserialize_sketch(func_name, binary.value(i))?;
        union.update(&sketch);
    }
    Ok(())
}
