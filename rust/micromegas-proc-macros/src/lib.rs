//! Top-level procedural macros for micromegas
//!
//! This crate provides high-level procedural macros that integrate multiple
//! micromegas components for a seamless developer experience.

use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// micromegas_main: Creates a tokio runtime with proper micromegas tracing callbacks and telemetry setup
///
/// This is a drop-in replacement for `#[tokio::main]` that automatically configures:
/// - Tokio runtime with proper micromegas tracing thread lifecycle callbacks  
/// - Telemetry guard with sensible defaults (ctrl-c handling, debug level)
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
/// ```
#[proc_macro_attribute]
pub fn micromegas_main(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    assert!(args.is_empty());
    let function = parse_macro_input!(input as ItemFn);

    // Ensure the function is async and named main
    if function.sig.asyncness.is_none() {
        panic!("micromegas_main can only be applied to async functions");
    }

    if function.sig.ident != "main" {
        panic!("micromegas_main can only be applied to the main function");
    }

    let original_block = &function.block;
    let return_type = &function.sig.output;

    let expanded = quote! {
        fn main() #return_type {
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
                // Set up telemetry guard with sensible defaults
                let _telemetry_guard = micromegas::telemetry_sink::TelemetryGuardBuilder::default()
                    .with_ctrlc_handling()
                    .with_local_sink_max_level(micromegas::tracing::levels::LevelFilter::Debug)
                    .build();

                // Execute the original main function body
                #original_block
            })
        }
    };

    proc_macro::TokenStream::from(expanded)
}
