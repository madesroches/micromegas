use convert_case::{Case, Casing};
use lazy_static::lazy_static;
use regex::{Captures, Regex};

pub use crate::filters::*;
use crate::{
    api_types::{Model, ModelOrigin, ModulePath, Parameter, Path, Type},
    errors::Error,
};

use super::TypeScriptGenerationContext;

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_type(
    type_: &Type,
    ctx: &TypeScriptGenerationContext,
    module_path: &ModulePath,
) -> ::askama::Result<String> {
    Ok(match type_ {
        Type::Any => "any".to_string(),
        Type::Int32 | Type::UInt32 | Type::Float32 => "number".to_string(),
        Type::Int64 | Type::UInt64 | Type::Float64 => "bigint".to_string(),
        Type::String => "string".to_string(),
        Type::Boolean => "boolean".to_string(),
        Type::Bytes | Type::Binary => "Blob".to_string(),
        Type::DateTime | Type::Date => "Date".to_string(),
        Type::Array(inner) => format!("{}[]", fmt_type(inner, ctx, module_path).unwrap()),
        Type::HashSet(inner) => format!("Set<{}>", fmt_type(inner, ctx, module_path).unwrap()),
        Type::Map(inner) => format!(
            "Record<string, {}>",
            fmt_type(inner, ctx, module_path).unwrap()
        ),
        Type::Named(ref_) => {
            let ref_module_path = ctx.ref_loc_to_typescript_module_path(ref_.ref_location())?;

            fmt_module_path(
                &ref_module_path
                    .relative_to(module_path)
                    .join(fmt_model_name(ctx.get_model(ref_)?, ctx)?),
            )?
        }
        Type::Enum { .. } | Type::Struct { .. } | Type::OneOf { .. } => {
            return Err(askama::Error::Custom(Box::new(Error::UnsupportedType(
                "complex types cannot be formatted".to_string(),
            ))))
        }
        Type::Box(inner) => fmt_type(inner.as_ref(), ctx, module_path)?,
    })
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_module_path(module_path: &ModulePath) -> ::askama::Result<String> {
    Ok(module_path
        .parts()
        .iter()
        .filter_map(|name| {
            if name == ".." {
                None
            } else {
                Some(name.to_case(Case::Pascal))
            }
        })
        .collect::<Vec<String>>()
        .join("."))
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_path(path: &Path, parameters: &[Parameter]) -> ::askama::Result<String> {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"\{([^/]+)\}").unwrap();
    }

    Ok(RE
        .replace_all(path.0.as_str(), |captures: &Captures<'_>| {
            let capture_name = captures.get(1).map(|match_| match_.as_str());

            if let Some(capture_name) = capture_name {
                if parameters
                    .iter()
                    .any(|parameter| parameter.name == capture_name)
                {
                    return format!("${{params[\"{}\"]}}", capture_name.to_case(Case::Camel));
                }
            }

            "unknown".to_string()
        })
        .to_string())
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_model_name(
    model: &Model,
    ctx: &TypeScriptGenerationContext,
) -> ::askama::Result<String> {
    let name = match &model.origin {
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
    };

    Ok(name.to_case(Case::Pascal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_path() {
        assert_eq!(
            fmt_path(
                &"/v1/users/{id}".into(),
                &[Parameter {
                    name: "id".into(),
                    description: None,
                    type_: Type::String,
                    required: true,
                }]
            )
            .unwrap(),
            "/v1/users/${params[\"id\"]}"
        );
        assert_eq!(
            fmt_path(
                &"/v1/users/{my-id}".into(),
                &[Parameter {
                    name: "my-id".into(),
                    description: None,
                    type_: Type::String,
                    required: true,
                }]
            )
            .unwrap(),
            "/v1/users/${params[\"myId\"]}"
        );
        assert_eq!(
            fmt_path(
                &"/v1/users/{id}/{name}".into(),
                &[
                    Parameter {
                        name: "id".into(),
                        description: None,
                        type_: Type::String,
                        required: true,
                    },
                    Parameter {
                        name: "name".into(),
                        description: None,
                        type_: Type::String,
                        required: true,
                    }
                ]
            )
            .unwrap(),
            "/v1/users/${params[\"id\"]}/${params[\"name\"]}"
        );
    }
}
