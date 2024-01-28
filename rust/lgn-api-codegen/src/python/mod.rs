use std::path::Path;

use crate::{
    api_types::{GenerationContext, MediaType, Type},
    Language, Result,
};
use askama::Template;
use lazy_static::__Deref;

mod filters;

#[derive(askama::Template)]
#[template(path = "__init__.py.jinja", escape = "none")]
struct PythonTemplate {
    pub ctx: GenerationContext,
}

impl Language {
    pub(crate) fn generate_python(ctx: GenerationContext, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;

        let output_file = output_dir.join("api.py");
        let content = generate_content(ctx)?;

        std::fs::write(output_file, content)?;

        Ok(())
    }
}

fn generate_content(ctx: GenerationContext) -> Result<String> {
    let content = PythonTemplate { ctx }.render()?;

    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{openapi_loader::OpenApiRefLocation, visitor::Visitor, OpenApiLoader};

    use super::*;

    #[test]
    fn test_py_generation() {
        let loader = OpenApiLoader::default();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/api-codegen")
            .canonicalize()
            .unwrap();
        let openapi = loader
            .load_openapi(OpenApiRefLocation::new(&root, "cars.yaml".into()))
            .unwrap();
        let ctx = Visitor::new(root).visit(&[openapi.clone()]).unwrap();
        let content = generate_content(ctx).unwrap();

        insta::assert_snapshot!(content);
    }
}
