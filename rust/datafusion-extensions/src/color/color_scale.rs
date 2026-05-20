use datafusion::arrow::array::{Array, Float64Array, StringArray, UInt32Builder};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::{Result, internal_err};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::{
    ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature, Volatility,
};
use datafusion::scalar::ScalarValue;
use std::any::Any;
use std::sync::Arc;

use super::{float_to_byte, pack_rgba};

/// Recognized colormap names. Listed in the error message so users can fix a
/// typo without consulting the docs.
const RECOGNIZED: &[&str] = &["viridis", "magma", "plasma", "inferno", "cividis", "turbo"];

/// Resolve a colormap name to a `colorous::Gradient`. Case-insensitive.
///
/// `colorous::Gradient` is `Copy` and the gradient items are `pub const` (not
/// `'static`), so this returns by value.
fn resolve_colormap(name: &str) -> Option<colorous::Gradient> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "viridis" => Some(colorous::VIRIDIS),
        "magma" => Some(colorous::MAGMA),
        "plasma" => Some(colorous::PLASMA),
        "inferno" => Some(colorous::INFERNO),
        "cividis" => Some(colorous::CIVIDIS),
        "turbo" => Some(colorous::TURBO),
        _ => None,
    }
}

fn unknown_colormap_err(name: &str) -> DataFusionError {
    DataFusionError::Execution(format!(
        "color_scale(): unknown colormap '{name}'. Recognized: {}",
        RECOGNIZED.join(", ")
    ))
}

/// `color_scale(name, t, alpha) -> UInt32`
///
/// Samples a built-in perceptually-uniform color scale (viridis, magma,
/// plasma, inferno, cividis, turbo) at position `t` and packs the result with
/// the user-supplied alpha as a `0xRRGGBBAA` `u32`. Both `t` and `alpha` are
/// clamped to `[0.0, 1.0]`. Unknown names raise an error.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct ColorScaleUdf {
    signature: Signature,
}

impl ColorScaleUdf {
    pub fn new() -> Self {
        Self {
            signature: Signature::exact(
                vec![DataType::Utf8, DataType::Float64, DataType::Float64],
                Volatility::Immutable,
            ),
        }
    }
}

impl Default for ColorScaleUdf {
    fn default() -> Self {
        Self::new()
    }
}

impl ScalarUDFImpl for ColorScaleUdf {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn name(&self) -> &str {
        "color_scale"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _args: &[DataType]) -> Result<DataType> {
        Ok(DataType::UInt32)
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        if args.args.len() != 3 {
            return internal_err!("wrong number of arguments to color_scale()");
        }

        // Fast path: literal colormap name. Resolve once, before lowering
        // the columns. Under `Volatility::Immutable` a fully-literal call is
        // constant-folded by DataFusion's `ConstEvaluator`, so an unknown
        // name surfaces at plan time. A column-driven `t` / `alpha` with a
        // literal `name` is not foldable, but this still produces a single
        // upfront error rather than per-row work.
        let literal_gradient = match &args.args[0] {
            ColumnarValue::Scalar(ScalarValue::Utf8(Some(s)))
            | ColumnarValue::Scalar(ScalarValue::LargeUtf8(Some(s)))
            | ColumnarValue::Scalar(ScalarValue::Utf8View(Some(s))) => {
                Some(resolve_colormap(s).ok_or_else(|| unknown_colormap_err(s))?)
            }
            ColumnarValue::Scalar(ScalarValue::Utf8(None))
            | ColumnarValue::Scalar(ScalarValue::LargeUtf8(None))
            | ColumnarValue::Scalar(ScalarValue::Utf8View(None)) => {
                // NULL name => NULL output for all rows; fall through and
                // the per-row null check will short-circuit.
                None
            }
            _ => None,
        };

        let arrays = ColumnarValue::values_to_arrays(&args.args)?;
        let names = arrays[0]
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| {
                DataFusionError::Internal("color_scale(): first argument must be Utf8".into())
            })?;
        let ts = arrays[1]
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                DataFusionError::Internal("color_scale(): second argument must be Float64".into())
            })?;
        let alphas = arrays[2]
            .as_any()
            .downcast_ref::<Float64Array>()
            .ok_or_else(|| {
                DataFusionError::Internal("color_scale(): third argument must be Float64".into())
            })?;

        let len = names.len();
        if ts.len() != len || alphas.len() != len {
            return internal_err!("arrays of different lengths in color_scale()");
        }

        let mut builder = UInt32Builder::with_capacity(len);
        for i in 0..len {
            if names.is_null(i) || ts.is_null(i) || alphas.is_null(i) {
                builder.append_null();
                continue;
            }
            let gradient = match literal_gradient {
                Some(g) => g,
                None => {
                    let name = names.value(i);
                    match resolve_colormap(name) {
                        Some(g) => g,
                        None => return Err(unknown_colormap_err(name)),
                    }
                }
            };
            let t = ts.value(i).clamp(0.0, 1.0);
            let color = gradient.eval_continuous(t);
            let a = float_to_byte(alphas.value(i));
            builder.append_value(pack_rgba(color.r, color.g, color.b, a));
        }
        Ok(ColumnarValue::Array(Arc::new(builder.finish())))
    }
}

/// Creates the `color_scale(name, t, alpha) -> UInt32` UDF.
pub fn make_color_scale_udf() -> ScalarUDF {
    ScalarUDF::new_from_impl(ColorScaleUdf::new())
}
