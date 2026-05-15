use datafusion::arrow::array::{Array, Float64Array, UInt32Array, UInt32Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

use super::{pack_rgba, round_to_byte, unpack_rgba};

/// `lerp_color(c1, c2, t) -> UInt32`
///
/// Component-wise linear interpolation between two packed RGBA `u32`s with
/// `t` in `[0.0, 1.0]`. `t` is clamped before interpolation; alpha is
/// interpolated alongside RGB (straight alpha, no premultiplication).
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct LerpColorUdf {
    signature: Signature,
}

impl LerpColorUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::UInt32, DataType::UInt32, DataType::Float64],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for LerpColorUdf {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
fn lerp_channel(a: u8, b: u8, t: f64) -> u8 {
    let af = a as f64;
    let bf = b as f64;
    round_to_byte(af + (bf - af) * t)
}

impl ScalarUDFImpl for LerpColorUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "lerp_color"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::UInt32)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 3 {
            return internal_err!("wrong number of arguments to lerp_color()");
        }

        let c1 = args[0]
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| {
                DataFusionError::Internal("lerp_color(): first argument must be UInt32".into())
            })?;
        let c2 = args[1]
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| {
                DataFusionError::Internal("lerp_color(): second argument must be UInt32".into())
            })?;
        let ts = args[2]
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                DataFusionError::Internal("lerp_color(): third argument must be Float64".into())
            })?;

        let len = c1.len();
        if c2.len() != len || ts.len() != len {
            return internal_err!("arrays of different lengths in lerp_color()");
        }

        let mut builder = UInt32Builder::with_capacity(len);
        for i in 0..len {
            if c1.is_null(i) || c2.is_null(i) || ts.is_null(i) {
                builder.append_null();
                continue;
            }
            let (r1, g1, b1, a1) = unpack_rgba(c1.value(i));
            let (r2, g2, b2, a2) = unpack_rgba(c2.value(i));
            let t = ts.value(i).clamp(0.0, 1.0);
            let r = lerp_channel(r1, r2, t);
            let g = lerp_channel(g1, g2, t);
            let b = lerp_channel(b1, b2, t);
            let a = lerp_channel(a1, a2, t);
            builder.append_value(pack_rgba(r, g, b, a));
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `lerp_color(c1, c2, t) -> UInt32` UDF.
pub fn make_lerp_color_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(LerpColorUdf::new())
}
