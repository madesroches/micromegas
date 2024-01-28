use std::{
    borrow::Cow,
    fs::{read_to_string, File},
    io::Write,
    path::Path,
    process::Command,
};

use anyhow::anyhow;
use askama::Template;
use tempfile::tempdir;
use which::which;

use crate::{
    api_types::{GenerationContext, MediaType, Type},
    errors::Error,
    Language, Result, TypeScriptOptions,
};

mod filters;

#[derive(askama::Template)]
#[template(path = "index.ts.jinja", escape = "none")]
struct TypeScriptIndexTemplate;

#[derive(askama::Template)]
#[template(path = "api.ts.jinja", escape = "none")]
struct TypeScriptApiTemplate<'a> {
    pub ctx: &'a TypeScriptGenerationContext,
}

#[derive(askama::Template)]
#[template(path = "package.json.jinja", escape = "none")]
struct TypeScriptPackageTemplate;

pub type TypeScriptGenerationContext = GenerationContext<TypeScriptOptions>;

fn format_typescript<'a, 'b>(
    options: &TypeScriptOptions,
    content: &'a str,
    temp_file_name: &'b str,
) -> Result<Cow<'a, str>> {
    let prettier_config_path = options
        .prettier_config_path
        .clone()
        .map(|path| path.to_string_lossy().into_owned());

    let dir =
        tempdir().map_err(|_err| Error::TypeScriptFormat(anyhow!("couldn't create a tmp dir")))?;

    let file_path = dir.path().join(temp_file_name);

    let mut file = File::create(&file_path)?;

    write!(file, "{}", content)
        .map_err(|_err| Error::TypeScriptFormat(anyhow!("couldn't write content to tmp file")))?;

    let binary_path = match which("npx") {
        Err(_) => return Ok(content.into()),
        Ok(path) => path,
    };

    let mut command = Command::new(binary_path);

    let tmp_file_path = file_path.as_path().to_string_lossy();

    let mut args = vec![
        "--yes",
        "prettier",
        "--loglevel",
        "silent",
        "--write",
        &tmp_file_path,
    ];

    match prettier_config_path {
        None => args.push("--no-config"),
        Some(ref path) => {
            args.push("--config");
            args.push(path);
        }
    };

    command.args(&args);

    let status = command.status().map_err(|error| {
        Error::TypeScriptFormat(anyhow!(
            "npx prettier command was not successful: {}",
            error
        ))
    })?;

    if status.success() {
        let formatted_content = read_to_string(&file_path)
            .map_err(|_err| Error::TypeScriptFormat(anyhow!("couldn't read tmp file")))?;

        Ok(formatted_content.into())
    } else {
        Err(Error::TypeScriptFormat(anyhow!(
            "npx prettier command was not successful: {}",
            status
        )))
    }
}

fn generate_index_content(ctx: &TypeScriptGenerationContext) -> Result<String> {
    let mut content = TypeScriptIndexTemplate.render()?;

    if !ctx.options.skip_format {
        content = format_typescript(&ctx.options, &content, "index.ts")?.into_owned();
    }

    Ok(content)
}

fn generate_api_content(ctx: &TypeScriptGenerationContext) -> Result<String> {
    let mut content = TypeScriptApiTemplate { ctx }.render()?;

    if !ctx.options.skip_format {
        content = format_typescript(&ctx.options, &content, "index.ts")?.into_owned();
    }

    Ok(content)
}

fn generate_package_content(options: &TypeScriptOptions) -> Result<String> {
    let mut content = TypeScriptPackageTemplate.render()?;

    if !options.skip_format {
        content = format_typescript(options, &content, "package.json")?.into_owned();
    }

    Ok(content)
}

impl Language {
    pub(crate) fn generate_typescript(
        ctx: GenerationContext,
        options: TypeScriptOptions,
        output_dir: &Path,
    ) -> Result<()> {
        if &options.filename == "index" {
            return Err(Error::TypeScriptFilename);
        }

        std::fs::create_dir_all(output_dir)?;

        let ctx = ctx.with_options(options);

        let output_file = output_dir.join(&ctx.options.filename).with_extension("ts");
        let content = generate_api_content(&ctx)?;
        std::fs::write(output_file, content)?;

        let output_file = output_dir.join("index").with_extension("ts");
        let content = generate_index_content(&ctx)?;
        std::fs::write(output_file, content)?;

        if ctx.options.with_package_json {
            let output_file = output_dir.join("package.json");
            let content = generate_package_content(&ctx.options)?;

            std::fs::write(output_file, content)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{openapi_loader::OpenApiRefLocation, visitor::Visitor, OpenApiLoader};

    use super::*;

    #[test]
    fn test_ts_index_generation() {
        let loader = OpenApiLoader::default();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/api-codegen")
            .canonicalize()
            .unwrap();
        let openapi = loader
            .load_openapi(OpenApiRefLocation::new(&root, "cars.yaml".into()))
            .unwrap();
        let ctx = Visitor::new(root)
            .visit(&[openapi.clone()])
            .unwrap()
            .with_options(TypeScriptOptions::default());
        let content = generate_index_content(&ctx).unwrap();

        insta::assert_snapshot!(content);
    }

    #[test]
    fn test_ts_api_generation() {
        let loader = OpenApiLoader::default();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/api-codegen")
            .canonicalize()
            .unwrap();
        let openapi = loader
            .load_openapi(OpenApiRefLocation::new(&root, "cars.yaml".into()))
            .unwrap();
        let ctx = Visitor::new(root)
            .visit(&[openapi.clone()])
            .unwrap()
            .with_options(TypeScriptOptions::default());
        let content = generate_api_content(&ctx).unwrap();

        insta::assert_snapshot!(content);
    }

    #[test]
    fn test_ts_package_generation() {
        let content = generate_package_content(&TypeScriptOptions::default()).unwrap();

        insta::assert_snapshot!(content);
    }
}
