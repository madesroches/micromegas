use datafusion::arrow::array::{Array, Float64Array, Float64Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

/// `lerp(a, b, t) -> Float64`
///
/// Linear interpolation between `a` and `b`. Computes `a + (b - a) * t`.
/// No clamping — `t` outside `[0, 1]` extrapolates past the endpoints.
/// See module docs for conventions and pathological-input behaviour.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct LerpUdf {
    signature: Signature,
}

impl LerpUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Float64, DataType::Float64, DataType::Float64],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for LerpUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for LerpUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "lerp"
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
            return internal_err!("wrong number of arguments to lerp()");
        }

        let inputs: Vec<&Float64Array> = args
            .iter()
            .map(|a| {
                a.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
                    DataFusionError::Internal("lerp(): expected Float64 inputs".into())
                })
            })
            .collect::<Result<_>>()?;

        let len = inputs[0].len();
        if inputs[1].len() != len || inputs[2].len() != len {
            return internal_err!("arrays of different lengths in lerp()");
        }

        let a = inputs[0];
        let b = inputs[1];
        let t = inputs[2];

        let mut builder = Float64Builder::with_capacity(len);
        for i in 0..len {
            if a.is_null(i) || b.is_null(i) || t.is_null(i) {
                builder.append_null();
            } else {
                let av = a.value(i);
                let bv = b.value(i);
                let tv = t.value(i);
                builder.append_value(av + (bv - av) * tv);
            }
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `lerp(a, b, t) -> Float64` UDF.
pub fn make_lerp_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(LerpUdf::new())
}
