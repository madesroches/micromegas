use datafusion::arrow::array::{Array, Float64Array, UInt32Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

use super::{float_to_byte, pack_rgba};

/// `rgba(r, g, b, a) -> UInt32`
///
/// Packs four `[0.0, 1.0]` floats into a `0xRRGGBBAA` `u32`. Each channel is
/// scaled to `0..=255` with round-half-up; out-of-range and non-finite inputs
/// are clamped at the byte boundary.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct RgbaUdf {
    signature: Signature,
}

impl RgbaUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                    DataType::Float64,
                ],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for RgbaUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for RgbaUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "rgba"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::UInt32)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 4 {
            return internal_err!("wrong number of arguments to rgba()");
        }

        let channels: Vec<&Float64Array> = args
            .iter()
            .map(|a| {
                a.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
                    DataFusionError::Internal("rgba(): expected Float64 inputs".into())
                })
            })
            .collect::<Result<_>>()?;

        let len = channels[0].len();
        for c in &channels[1..] {
            if c.len() != len {
                return internal_err!("arrays of different lengths in rgba()");
            }
        }

        let mut builder = UInt32Builder::with_capacity(len);
        for i in 0..len {
            if channels.iter().any(|c| c.is_null(i)) {
                builder.append_null();
            } else {
                let r = float_to_byte(channels[0].value(i));
                let g = float_to_byte(channels[1].value(i));
                let b = float_to_byte(channels[2].value(i));
                let a = float_to_byte(channels[3].value(i));
                builder.append_value(pack_rgba(r, g, b, a));
            }
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `rgba(r, g, b, a) -> UInt32` UDF.
pub fn make_rgba_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(RgbaUdf::new())
}
