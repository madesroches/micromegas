pub use crate::filters::*;
use crate::{
    api_types::{Model, ModelOrigin, ModulePath, Parameter, Path, Type},
    errors::Error,
};
use convert_case::{Case, Casing};
use lazy_static::lazy_static;
use regex::Regex;

use super::RustGenerationContext;

const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl", "in",
    "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "Self", "static", "struct", "super", "trait", "true", "try", "type",
    "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_model_name(model: &Model, ctx: &RustGenerationContext) -> ::askama::Result<String> {
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
pub fn fmt_type(
    type_: &Type,
    ctx: &RustGenerationContext,
    module_path: &ModulePath,
) -> ::askama::Result<String> {
    Ok(match type_ {
        Type::Any => "serde_json::Value".to_string(),
        Type::Int32 => "i32".to_string(),
        Type::Int64 => "i64".to_string(),
        Type::UInt32 => "u32".to_string(),
        Type::UInt64 => "u64".to_string(),
        Type::String => "String".to_string(),
        Type::Boolean => "bool".to_string(),
        Type::Float32 => "f32".to_string(),
        Type::Float64 => "f64".to_string(),
        Type::Bytes | Type::Binary => "lgn_online::codegen::Bytes".to_string(),
        Type::DateTime => "chrono::DateTime::<chrono::Utc>".to_string(),
        Type::Date => "chrono::Date::<chrono::Utc>".to_string(),
        Type::Array(inner) => format!("Vec<{}>", fmt_type(inner, ctx, module_path)?),
        Type::HashSet(inner) => format!(
            "std::collections::HashSet<{}>",
            fmt_type(inner, ctx, module_path).unwrap()
        ),
        Type::Map(inner) => format!(
            "std::collections::BTreeMap<String, {}>",
            fmt_type(inner, ctx, module_path).unwrap()
        ),
        Type::Named(ref_) => {
            // We need to compute the relative module path.

            let ref_module_path = ctx.ref_loc_to_rust_module_path(ref_.ref_location())?;

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
        Type::Box(inner) => format!("Box<{}>", fmt_type(inner, ctx, module_path)?),
    })
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_module_path(module_path: &ModulePath) -> ::askama::Result<String> {
    Ok(module_path.to_rust_module_path())
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_field(name: &str) -> ::askama::Result<String> {
    if is_keyword(name) {
        Ok(format!("{}_", snake_case(name).unwrap()))
    } else {
        Ok(snake_case(name).unwrap())
    }
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_axum_path(path: &Path) -> ::askama::Result<String> {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"\{(?P<p>[^/]+)\}").unwrap();
    }

    Ok(RE.replace_all(path.0.as_str(), ":$p").to_string())
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_rust_path(path: &Path) -> ::askama::Result<String> {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"\{([^/]+)\}").unwrap();
    }

    Ok(RE.replace_all(path.0.as_str(), "{}").to_string())
}

#[allow(clippy::unnecessary_wraps)]
pub fn join_names(params: &[Parameter]) -> ::askama::Result<String> {
    let joined = params
        .iter()
        .map(|param| snake_case(param.name.clone()).unwrap())
        .collect::<Vec<String>>()
        .join(", ");

    if params.len() > 1 {
        Ok(format!("({})", joined))
    } else {
        Ok(joined)
    }
}

#[allow(clippy::unnecessary_wraps)]
pub fn join_types(
    params: &[Parameter],
    ctx: &RustGenerationContext,
    module_path: &ModulePath,
) -> ::askama::Result<String> {
    let joined = params
        .iter()
        .map(|param| {
            let type_ = fmt_type(&param.type_, ctx, module_path).unwrap();
            if param.required {
                type_
            } else {
                format!("Option<{}>", type_)
            }
        })
        .collect::<Vec<String>>()
        .join(", ");

    if params.len() > 1 {
        Ok(format!("({})", joined))
    } else {
        Ok(joined)
    }
}

#[allow(clippy::unnecessary_wraps)]
pub fn fmt_struct_derive(type_: &Option<Box<Type>>) -> ::askama::Result<String> {
    Ok(match type_ {
        Some(type_) if matches!(**type_, Type::Any) => {
            "#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]"
        }
        _ => "#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]",
    }
    .to_string())
}

fn is_keyword(name: &str) -> bool {
    KEYWORDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{api_types::GenerationContext, RustOptions};

    use super::*;

    #[test]
    fn test_join_names() {
        let p1 = Parameter {
            name: "foo".to_string(),
            description: None,
            type_: Type::Int32,
            required: true,
        };
        let p2 = Parameter {
            name: "bar".to_string(),
            description: None,
            type_: Type::String,
            required: false,
        };
        assert_eq!(join_names(&[p1.clone()]).unwrap(), "foo".to_string());
        assert_eq!(join_names(&[p1, p2]).unwrap(), "(foo, bar)".to_string());
    }

    #[test]
    fn test_join_types() {
        let ctx = GenerationContext::new(PathBuf::from("/")).with_options(RustOptions::default());
        let module_path = "foo/bar".parse().unwrap();

        let p1 = Parameter {
            name: "foo".to_string(),
            description: None,
            type_: Type::Int32,
            required: true,
        };
        let p2 = Parameter {
            name: "bar".to_string(),
            description: None,
            type_: Type::String,
            required: false,
        };
        assert_eq!(
            join_types(&[p1.clone()], &ctx, &module_path).unwrap(),
            "i32".to_string()
        );
        assert_eq!(
            join_types(&[p1, p2], &ctx, &module_path).unwrap(),
            "(i32, Option<String>)".to_string()
        );
    }

    #[test]
    fn test_fmt_axum_path() {
        assert_eq!(
            fmt_axum_path(&"/v1/users/{id}".into()).unwrap(),
            "/v1/users/:id"
        );
        assert_eq!(
            fmt_axum_path(&"/v1/users/:id".into()).unwrap(),
            "/v1/users/:id"
        );
        assert_eq!(
            fmt_axum_path(&"/v1/users/{my-id}".into()).unwrap(),
            "/v1/users/:my-id"
        );
        assert_eq!(
            fmt_axum_path(&"/v1/users/{id}/{name}".into()).unwrap(),
            "/v1/users/:id/:name"
        );
    }

    #[test]
    fn test_fmt_rust_path() {
        assert_eq!(
            fmt_rust_path(&"/v1/users/{id}".into()).unwrap(),
            "/v1/users/{}"
        );
        assert_eq!(
            fmt_rust_path(&"/v1/users/{my-id}".into()).unwrap(),
            "/v1/users/{}"
        );
        assert_eq!(
            fmt_rust_path(&"/v1/users/{id}/{name}".into()).unwrap(),
            "/v1/users/{}/{}"
        );
    }
}
