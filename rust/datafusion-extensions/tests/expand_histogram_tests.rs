use datafusion::arrow::datatypes::DataType;
use datafusion::catalog::TableFunctionImpl;
use datafusion::logical_expr::Cast;
use datafusion::prelude::Expr;
use datafusion::scalar::ScalarValue;
use micromegas_datafusion_extensions::histogram::expand::ExpandHistogramTableFunction;

#[test]
fn test_call_accepts_cast_expression() {
    let func = ExpandHistogramTableFunction::new();
    // Construct a Cast expression — neither Literal nor ScalarSubquery
    let inner = Expr::Literal(ScalarValue::Null, None);
    let cast_expr = Expr::Cast(Cast::new(Box::new(inner), DataType::Null));
    let result = func.call(&[cast_expr]);
    assert!(
        result.is_ok(),
        "call() should accept Cast expression, got: {result:?}"
    );
}
