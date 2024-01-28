use crate::{
    api_types::{GenerationContext, Model, ModelOrigin, Path, Type},
    errors::Error,
};

pub use crate::filters::*;

use convert_case::{Case, Casing};
use lazy_static::lazy_static;
use regex::Regex;

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_model_name(model: &Model, ctx: &GenerationContext) -> ::askama::Result<String> {
    Ok(match &model.origin {
        ModelOrigin::Schemas => model.ref_.json_pointer().type_name().to_string(),
        ModelOrigin::ObjectProperty { object_pointer } => {
            format!(
                "{}_{}",
                fmt_model_name(
                    ctx.get_model(&model.ref_.clone().with_json_pointer(object_pointer.clone()))?,
                    ctx,
                )?,
                model.ref_.json_pointer().type_name()
            )
        }
        ModelOrigin::RequestBody { operation_name } => {
            format!("{}_body", operation_name)
        }
        ModelOrigin::ResponseBody {
            operation_name,
            status_code,
        } => format!("{}_{}_response", operation_name, status_code),
    }
    .to_case(Case::Pascal))
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_type(type_: &Type, ctx: &GenerationContext) -> ::askama::Result<String> {
    Ok(match type_ {
        Type::Any => "Any".to_string(),
        Type::Int32 | Type::Int64 | Type::UInt32 | Type::UInt64 => "int".to_string(),
        Type::String | Type::Bytes | Type::Binary => "str".to_string(), // at the moment the binary is passed as string
        Type::Boolean => "bool".to_string(),
        Type::Float32 | Type::Float64 => "float".to_string(),
        Type::DateTime => "datetime".to_string(),
        Type::Date => "date".to_string(),
        Type::Array(inner) | Type::HashSet(inner) => {
            format!("list[{}]", fmt_type(inner, ctx).unwrap())
        }
        Type::Map(inner) => format!("dict[string, {}]", fmt_type(inner, ctx).unwrap()),
        Type::Named(ref_) => fmt_model_name(ctx.get_model(ref_)?, ctx)?,
        Type::Enum { .. } | Type::Struct { .. } | Type::OneOf { .. } => {
            return Err(askama::Error::Custom(Box::new(Error::UnsupportedType(
                "complex types cannot be formatted".to_string(),
            ))))
        }
        Type::Box(inner) => fmt_type(inner.as_ref(), ctx)?,
    })
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_py_path(path: &Path) -> ::askama::Result<String> {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"\{([^/]+)\}").unwrap();
    }

    Ok(RE.replace_all(path.0.as_str(), "{}").to_string())
}
