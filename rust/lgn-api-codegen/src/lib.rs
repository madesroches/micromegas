//! Legion `OpenApi` code generator crate.
//!
//! Provides code generation for various languages based on an `OpenAPI` v3 specification.
//!

// crate-specific lint exceptions:
//#![allow()]

pub(crate) mod api_types;
pub mod errors;
pub(crate) mod filters;
pub(crate) mod openapi_loader;
pub(crate) mod python;
pub(crate) mod rust;
pub(crate) mod typescript;
pub(crate) mod visitor;

pub use api_types::{
    Language, ModulePath, RustOptions, TypeScriptAliasMappings, TypeScriptOptions,
};
use errors::{Error, Result};
use openapi_loader::{OpenApi, OpenApiElement, OpenApiLoader, OpenApiRefLocation};
use std::path::{Path, PathBuf};
use visitor::Visitor;

/// Generates the code for the specificed language and the specified APIs.
///
/// The code will look for each API using the specified name suffixed by `.yaml`
/// in the specified root.
///
/// If the root is a relative file, it will be resolved relative to the current
/// working directory.
///
/// # Errors
///
/// If the generation fails to complete.
pub fn generate(
    language: Language,
    root: impl AsRef<Path>,
    openapis: impl IntoIterator<Item = impl AsRef<str>>,
    output_dir: impl AsRef<Path>,
) -> Result<Vec<PathBuf>> {
    let root = if root.as_ref().is_relative() {
        std::env::current_dir()?.join(root)
    } else {
        root.as_ref().to_path_buf()
    }
    .canonicalize()?;

    let loader = OpenApiLoader::default();

    let openapis = openapis
        .into_iter()
        .map(|name| {
            let ref_loc =
                OpenApiRefLocation::new(&root, PathBuf::from(format!("{}.yaml", name.as_ref())));

            loader.load_openapi(ref_loc)
        })
        .collect::<Result<Vec<_>>>()?;

    let ctx = Visitor::new(root).visit(&openapis)?;

    language.generate(ctx, output_dir.as_ref())?;

    Ok(loader.get_all_files())
}

#[macro_export]
macro_rules! generate {
    ($lang:expr, $root:expr, $openapis:expr$(,)?) => {{
        match lgn_api_codegen::generate(
            $lang,
            $root,
            $openapis,
            std::env::var("OUT_DIR").expect("OUT_DIR must be set"),
        ) {
            Ok(files) => {
                for file in files {
                    println!("cargo:rerun-if-changed={}", file.display());
                }
            }
            Err(err) => {
                eprintln!("OpenAPI generation failed with: {}", err);
                panic!("OpenAPI generation failed");
            }
        }
    }};
}
