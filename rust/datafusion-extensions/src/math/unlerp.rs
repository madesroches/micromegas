use datafusion::arrow::array::{Array, Float64Array, Float64Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

/// `unlerp(a, b, x) -> Float64`
///
/// Inverse linear interpolation. Computes `(x - a) / (b - a)` — i.e. the
/// `t` such that `lerp(a, b, t) == x`. No clamping; `x` outside `[a, b]`
/// returns a value outside `[0, 1]`. When `a == b`, the result is IEEE
/// `NaN` (if `x == a`) or `±Inf` (if `x != a`); see module docs.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct UnlerpUdf {
    signature: Signature,
}

impl UnlerpUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Float64, DataType::Float64, DataType::Float64],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for UnlerpUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for UnlerpUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "unlerp"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Float64)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 3 {
            return internal_err!("wrong number of arguments to unlerp()");
        }

        let inputs: Vec<&Float64Array> = args
            .iter()
            .map(|a| {
                a.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
                    DataFusionError::Internal("unlerp(): expected Float64 inputs".into())
                })
            })
            .collect::<Result<_>>()?;

        let len = inputs[0].len();
        if inputs[1].len() != len || inputs[2].len() != len {
            return internal_err!("arrays of different lengths in unlerp()");
        }

        let a = inputs[0];
        let b = inputs[1];
        let x = inputs[2];

        let mut builder = Float64Builder::with_capacity(len);
        for i in 0..len {
            if a.is_null(i) || b.is_null(i) || x.is_null(i) {
                builder.append_null();
            } else {
                let av = a.value(i);
                let bv = b.value(i);
                let xv = x.value(i);
                builder.append_value((xv - av) / (bv - av));
            }
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `unlerp(a, b, x) -> Float64` UDF.
pub fn make_unlerp_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(UnlerpUdf::new())
}
