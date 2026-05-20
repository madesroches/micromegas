use datafusion::arrow::array::{Array, Float64Array, Float64Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use std::any::Any;
use std::sync::Arc;

/// `bin_center(coord, cell_size) -> Float64`
///
/// Snaps a coordinate to the center of its enclosing 1D bin. Bins are
/// centered on zero with width `cell_size`; the bin containing `coord`
/// spans the half-open interval `[c - cs/2, c + cs/2)` where `c` is the
/// returned center. See module docs for conventions and pathological-input
/// behaviour.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct BinCenterUdf {
    signature: Signature,
}

impl BinCenterUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Float64, DataType::Float64],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for BinCenterUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for BinCenterUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "bin_center"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::Float64)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let args = ColumnarValue::values_to_arrays(&args.args)?;
        if args.len() != 2 {
            return internal_err!("wrong number of arguments to bin_center()");
        }

        let inputs: Vec<&Float64Array> = args
            .iter()
            .map(|a| {
                a.as_any().downcast_ref::<Float64Array>().ok_or_else(|| {
                    DataFusionError::Internal("bin_center(): expected Float64 inputs".into())
                })
            })
            .collect::<Result<_>>()?;

        let len = inputs[0].len();
        if inputs[1].len() != len {
            return internal_err!("arrays of different lengths in bin_center()");
        }

        let coord = inputs[0];
        let cell_size = inputs[1];

        let mut builder = Float64Builder::with_capacity(len);
        for i in 0..len {
            if coord.is_null(i) || cell_size.is_null(i) {
                builder.append_null();
            } else {
                let c = coord.value(i);
                let cs = cell_size.value(i);
                builder.append_value(((c + cs * 0.5) / cs).floor() * cs);
            }
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `bin_center(coord, cell_size) -> Float64` UDF.
pub fn make_bin_center_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(BinCenterUdf::new())
}
