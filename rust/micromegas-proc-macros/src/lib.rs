//! Top-level procedural macros for micromegas
//!
//! This crate provides high-level procedural macros that integrate multiple
//! micromegas components for a seamless developer experience.

use quote::quote;
use syn::{AttributeArgs, ItemFn, Lit, Meta, NestedMeta, parse_macro_input};

/// micromegas_main: Creates a tokio runtime with proper micromegas tracing callbacks and telemetry setup
///
/// This is a drop-in replacement for `#[tokio::main]` that automatically configures:
/// - Tokio runtime with proper micromegas tracing thread lifecycle callbacks  
/// - Telemetry guard with sensible defaults (ctrl-c handling, debug level)
///
/// # Parameters
///
/// - `interop_max_level`: Optional interop max level override (e.g., "info", "debug", "warn")
/// - `max_level_override`: Optional max level override (e.g., "info", "debug", "warn")
///
/// # Examples
///
/// ```ignore
/// use micromegas::tracing::prelude::*;
///
/// #[micromegas_main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     info!("Server starting - telemetry already configured!");
///     Ok(())
/// }
///
/// #[micromegas_main(interop_max_level = "info")]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     info!("Server starting with info interop level!");
///     Ok(())
/// }
///
/// #[micromegas_main(max_level_override = "warn", interop_max_level = "info")]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     info!("Server starting with both level overrides!");
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn micromegas_main(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = parse_macro_input!(args as AttributeArgs);
    let function = parse_macro_input!(input as ItemFn);

    // Ensure the function is async and named main
    if function.sig.asyncness.is_none() {
        panic!("micromegas_main can only be applied to async functions");
    }

    if function.sig.ident != "main" {
        panic!("micromegas_main can only be applied to the main function");
    }

    // Parse the level override parameters if provided
    let mut interop_max_level: Option<String> = None;
    let mut max_level_override: Option<String> = None;

    for arg in args {
        match arg {
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("interop_max_level") => {
                if let Lit::Str(lit_str) = nv.lit {
                    interop_max_level = Some(lit_str.value());
                } else {
                    panic!("interop_max_level must be a string literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("max_level_override") => {
                if let Lit::Str(lit_str) = nv.lit {
                    max_level_override = Some(lit_str.value());
                } else {
                    panic!("max_level_override must be a string literal");
                }
            }
            _ => panic!(
                "Unsupported attribute argument. Supported: interop_max_level, max_level_override"
            ),
        }
    }

    let original_block = &function.block;
    let return_type = &function.sig.output;

    // Helper function to convert level string to LevelFilter token
    let level_to_filter = |level: &str| -> proc_macro2::TokenStream {
        match level.to_lowercase().as_str() {
            "trace" => quote! { micromegas::tracing::levels::LevelFilter::Trace },
            "debug" => quote! { micromegas::tracing::levels::LevelFilter::Debug },
            "info" => quote! { micromegas::tracing::levels::LevelFilter::Info },
            "warn" => quote! { micromegas::tracing::levels::LevelFilter::Warn },
            "error" => quote! { micromegas::tracing::levels::LevelFilter::Error },
            "off" => quote! { micromegas::tracing::levels::LevelFilter::Off },
            _ => {
                panic!("Invalid level value. Must be one of: trace, debug, info, warn, error, off")
            }
        }
    };

    // Generate the telemetry guard builder with optional level overrides
    let mut builder_calls = vec![
        quote! { .with_ctrlc_handling() },
        quote! { .with_local_sink_max_level(micromegas::tracing::levels::LevelFilter::Debug) },
        quote! { .with_process_property("version".to_string(), env!("CARGO_PKG_VERSION").to_string()) },
    ];

    if let Some(level) = max_level_override {
        let level_filter = level_to_filter(&level);
        builder_calls.push(quote! { .with_max_level_override(#level_filter) });
    }

    if let Some(level) = interop_max_level {
        let level_filter = level_to_filter(&level);
        builder_calls.push(quote! { .with_interop_max_level_override(#level_filter) });
    }

    let telemetry_guard_builder = quote! {
        micromegas::telemetry_sink::TelemetryGuardBuilder::default()
            #(#builder_calls)*
            .build()
    };

    let expanded = quote! {
        fn main() #return_type {
            // Set up telemetry guard BEFORE building tokio runtime
            // This ensures dispatch is initialized before worker threads start
            let _telemetry_guard = #telemetry_guard_builder;

            // Build the runtime with tracing callbacks
            let runtime = {
                use micromegas::tracing::runtime::TracingRuntimeExt;
                let mut builder = tokio::runtime::Builder::new_multi_thread();
                builder.enable_all();
                builder.thread_name(env!("CARGO_PKG_NAME"));
                builder.with_tracing_callbacks();
                builder.build().expect("Failed to build tokio runtime with tracing callbacks")
            };

            runtime.block_on(async move {
                // Execute the original main function body
                #original_block
            })
        }
    };

    proc_macro::TokenStream::from(expanded)
}
