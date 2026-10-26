use super::{HllAccumulator, binary_type};
use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, BinaryArray, Float32Array, Float64Array, Int8Array, Int16Array,
    Int32Array, Int64Array, LargeBinaryArray, StringViewArray, UInt8Array, UInt16Array,
    UInt32Array, UInt64Array,
};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::common::plan_err;
use datafusion::error::{DataFusionError, Result};
use datafusion::logical_expr::{
    Accumulator, AggregateUDF, AggregateUDFImpl, Signature, Volatility, function::AccumulatorArgs,
    function::StateFieldsArgs, utils::format_state_name,
};
use datasketches::hash::value::{canonical_float, raw_bytes, sign_extend};
use datasketches::hll::HllUnion;
use std::sync::Arc;

const NAME: &str = "make_hll";

fn is_supported(data_type: &DataType) -> bool {
    match data_type {
        DataType::Dictionary(_, value_type) => is_supported(value_type),
        DataType::Utf8
        | DataType::LargeUtf8
        | DataType::Utf8View
        | DataType::Binary
        | DataType::LargeBinary
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float32
        | DataType::Float64 => true,
        _ => false,
    }
}

// Each value is wrapped in an explicit hash strategy because the default `Hash` impls for
// strings and slices add terminators/length prefixes that other DataSketches implementations
// do not, which would make sketches unmergeable with theirs.
fn add_values(union: &mut HllUnion, array: &ArrayRef) -> Result<()> {
    macro_rules! add_primitive {
        ($arr_ty:ty, $wrap:expr) => {{
            let arr = array.as_any().downcast_ref::<$arr_ty>().ok_or_else(|| {
                DataFusionError::Internal(format!("{NAME}: unexpected array type"))
            })?;
            for v in arr.iter().flatten() {
                union.update_value($wrap(v));
            }
        }};
    }
    macro_rules! add_str {
        ($iter:expr) => {{
            for v in $iter.flatten() {
                if !v.is_empty() {
                    union.update_value(raw_bytes::from_str(v));
                }
            }
        }};
    }
    macro_rules! add_bytes {
        ($iter:expr) => {{
            for v in $iter.flatten() {
                if !v.is_empty() {
                    union.update_value(raw_bytes::from_slice(v));
                }
            }
        }};
    }

    match array.data_type() {
        DataType::Dictionary(_, value_type) => {
            let unpacked = cast(array, value_type)?;
            return add_values(union, &unpacked);
        }
        DataType::Null => {}
        DataType::Utf8 => add_str!(array.as_string::<i32>().iter()),
        DataType::LargeUtf8 => add_str!(array.as_string::<i64>().iter()),
        DataType::Utf8View => {
            let arr = array
                .as_any()
                .downcast_ref::<StringViewArray>()
                .ok_or_else(|| {
                    DataFusionError::Internal(format!("{NAME}: unexpected array type"))
                })?;
            add_str!(arr.iter());
        }
        DataType::Binary => {
            let arr = array
                .as_any()
                .downcast_ref::<BinaryArray>()
                .ok_or_else(|| {
                    DataFusionError::Internal(format!("{NAME}: unexpected array type"))
                })?;
            add_bytes!(arr.iter());
        }
        DataType::LargeBinary => {
            let arr = array
                .as_any()
                .downcast_ref::<LargeBinaryArray>()
                .ok_or_else(|| {
                    DataFusionError::Internal(format!("{NAME}: unexpected array type"))
                })?;
            add_bytes!(arr.iter());
        }
        DataType::Int8 => add_primitive!(Int8Array, sign_extend::from_i8),
        DataType::Int16 => add_primitive!(Int16Array, sign_extend::from_i16),
        DataType::Int32 => add_primitive!(Int32Array, sign_extend::from_i32),
        DataType::UInt8 => add_primitive!(UInt8Array, sign_extend::from_u8),
        DataType::UInt16 => add_primitive!(UInt16Array, sign_extend::from_u16),
        DataType::UInt32 => add_primitive!(UInt32Array, sign_extend::from_u32),
        DataType::Int64 => add_primitive!(Int64Array, |v: i64| v),
        // Values above i64::MAX wrap; the hash only needs the 64 bits.
        DataType::UInt64 => add_primitive!(UInt64Array, |v: u64| v as i64),
        DataType::Float32 => add_primitive!(Float32Array, canonical_float::from_f32),
        DataType::Float64 => add_primitive!(Float64Array, canonical_float::from_f64),
        other => return plan_err!("{NAME}: unsupported input type {other}"),
    }
    Ok(())
}

/// Aggregate that builds a serialized HyperLogLog sketch from the values of a column.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct MakeHll {
    signature: Signature,
}

impl MakeHll {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(1, Volatility::Immutable),
        }
    }
}

impl Default for MakeHll {
    fn default() -> Self {
        Self::new()
    }
}

impl AggregateUDFImpl for MakeHll {
    fn name(&self) -> &str {
        NAME
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, arg_types: &[DataType]) -> Result<DataType> {
        match arg_types {
            [t] if is_supported(t) || matches!(t, DataType::Null) => Ok(binary_type()),
            [t] => plan_err!("{NAME}: unsupported input type {t}"),
            _ => plan_err!("{NAME} takes exactly one argument"),
        }
    }

    fn accumulator(&self, _args: AccumulatorArgs) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(HllAccumulator::new(NAME, add_values)?))
    }

    fn state_fields(&self, args: StateFieldsArgs) -> Result<Vec<FieldRef>> {
        Ok(vec![Arc::new(Field::new(
            format_state_name(args.name, NAME),
            binary_type(),
            true,
        ))])
    }
}

/// Creates the `make_hll` aggregate function.
pub fn make_hll_udaf() -> AggregateUDF {
    AggregateUDF::from(MakeHll::new())
}
