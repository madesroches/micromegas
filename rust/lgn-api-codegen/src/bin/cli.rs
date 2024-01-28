//! Legion `OpenApi` code generator CLI.
//!
//! Provides code generation commands.
//!

use std::path::PathBuf;

use clap::{ArgEnum, Parser};
use lgn_api_codegen::{
    generate, Language as InternalLanguage, RustOptions, TypeScriptAliasMappings, TypeScriptOptions,
};
use telemetry_sink::TelemetryGuardBuilder;
use tracing::LevelFilter;

#[derive(Debug, Copy, Clone, ArgEnum)]
pub enum Language {
    Rust,
    #[clap(name = "typescript")]
    TypeScript,
    Python,
}

#[derive(Parser, Debug)]
#[clap(name = "Legion API Code Generator")]
#[clap(
    about = "CLI to generate code based on an OpenAPI v3 specification.",
    version,
    author
)]
#[clap(arg_required_else_help(true))]
struct Args {
    #[clap(name = "debug", short, long, help = "Enable debug logging")]
    debug: bool,

    #[clap(
        arg_enum,
        name = "language",
        long,
        help = "The language to generate code for."
    )]
    language: Language,
    #[clap(name = "root", help = "The root where to find the APIs.")]
    root: PathBuf,
    #[clap(name = "openapis", help = "The OpenAPIs to generate the code for.")]
    openapis: Vec<String>,
    #[clap(
        name = "out-dir",
        long,
        env,
        help = "The directory to output generated code to."
    )]
    out_dir: PathBuf,

    // Languages specific options
    #[clap(
        long,
        env,
        help = "A custom Prettier config path (only works when targeting TypeScript)"
    )]
    prettier_config_path: Option<PathBuf>,
    #[clap(
        long,
        env,
        help = "Generates a package.json alongside the source files (only works when targeting TypeScript)"
    )]
    with_package_json: bool,
    #[clap(long, help = "Skip code format (only works when targeting TypeScript)")]
    skip_format: bool,
    #[clap(
        long,
        help = "Specify filename without prefix nor extension (only works when targeting TypeScript)"
    )]
    typescript_filename: String,
}

#[allow(clippy::let_unit_value)]
fn main() -> anyhow::Result<()> {
    let args: Args = Args::parse();

    let _telemetry_guard = TelemetryGuardBuilder::default()
        .with_local_sink_enabled(args.debug)
        .with_local_sink_max_level(LevelFilter::Debug)
        .build();

    let internal_language = match args.language {
        Language::Rust => InternalLanguage::Rust(RustOptions::default()),
        Language::TypeScript => InternalLanguage::TypeScript(TypeScriptOptions {
            alias_mappings: TypeScriptAliasMappings::default(),
            prettier_config_path: args.prettier_config_path.map(PathBuf::from),
            skip_format: args.skip_format,
            with_package_json: args.with_package_json,
            filename: args.typescript_filename,
        }),
        Language::Python => InternalLanguage::Python,
    };

    generate(internal_language, args.root, &args.openapis, &args.out_dir)?;

    Ok(())
}
