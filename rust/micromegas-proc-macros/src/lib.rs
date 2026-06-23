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
/// - Automatic authentication configuration from environment variables
///
/// # Authentication
///
/// The macro automatically configures telemetry authentication based on environment variables:
///
/// - **API Key:** Set `MICROMEGAS_INGESTION_API_KEY=your-key`
/// - **OIDC Client Credentials:** Set `MICROMEGAS_OIDC_TOKEN_ENDPOINT`, `MICROMEGAS_OIDC_CLIENT_ID`, `MICROMEGAS_OIDC_CLIENT_SECRET`
/// - **No auth:** If no env vars are set, telemetry is sent unauthenticated (requires `--disable-auth` on ingestion server)
///
/// # Parameters
///
/// - `ctrlc_handling`: bool (default: `true`) — enable Ctrl-C graceful shutdown
/// - `install_log_capture`: bool (default: `false`) — capture `log` crate output
/// - `interop_max_level`: string (e.g., `"info"`) — interop max level override
/// - `local_sink_enabled`: bool (default: `true`) — enable local stderr sink
/// - `local_sink_max_level`: string (default: `"debug"`) — max level for local sink
/// - `max_level_override`: string (e.g., `"warn"`) — global max level override
/// - `system_metrics`: bool (default: `true`) — collect system metrics
/// - `telemetry_url`: string — override the telemetry ingestion URL
/// - `api_key`: string — embed a literal API key (takes precedence over env-var auth)
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
///
/// #[micromegas_main(telemetry_url = "http://localhost:9000", api_key = "my-secret-key")]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     info!("Server with explicit URL and embedded API key!");
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

    // Parse attribute parameters
    let mut interop_max_level: Option<String> = None;
    let mut max_level_override: Option<String> = None;
    let mut ctrlc_handling: bool = true;
    let mut local_sink_enabled: bool = true;
    let mut local_sink_max_level: Option<String> = None;
    let mut install_log_capture: bool = false;
    let mut system_metrics: bool = true;
    let mut telemetry_url: Option<String> = None;
    let mut api_key: Option<String> = None;

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
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("ctrlc_handling") => {
                if let Lit::Bool(lit_bool) = nv.lit {
                    ctrlc_handling = lit_bool.value();
                } else {
                    panic!("ctrlc_handling must be a bool literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("local_sink_enabled") => {
                if let Lit::Bool(lit_bool) = nv.lit {
                    local_sink_enabled = lit_bool.value();
                } else {
                    panic!("local_sink_enabled must be a bool literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("local_sink_max_level") => {
                if let Lit::Str(lit_str) = nv.lit {
                    local_sink_max_level = Some(lit_str.value());
                } else {
                    panic!("local_sink_max_level must be a string literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("install_log_capture") => {
                if let Lit::Bool(lit_bool) = nv.lit {
                    install_log_capture = lit_bool.value();
                } else {
                    panic!("install_log_capture must be a bool literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("system_metrics") => {
                if let Lit::Bool(lit_bool) = nv.lit {
                    system_metrics = lit_bool.value();
                } else {
                    panic!("system_metrics must be a bool literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("telemetry_url") => {
                if let Lit::Str(lit_str) = nv.lit {
                    telemetry_url = Some(lit_str.value());
                } else {
                    panic!("telemetry_url must be a string literal");
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("api_key") => {
                if let Lit::Str(lit_str) = nv.lit {
                    api_key = Some(lit_str.value());
                } else {
                    panic!("api_key must be a string literal");
                }
            }
            _ => panic!(
                "Unsupported attribute argument. Supported: api_key, ctrlc_handling, install_log_capture, interop_max_level, local_sink_enabled, local_sink_max_level, max_level_override, system_metrics, telemetry_url"
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

    // Generate the telemetry guard builder
    let mut builder_calls = vec![quote! {
        .with_process_property("version".to_string(), env!("CARGO_PKG_VERSION").to_string())
    }];

    if ctrlc_handling {
        builder_calls.push(quote! { .with_ctrlc_handling() });
    }

    if !local_sink_enabled {
        builder_calls.push(quote! { .with_local_sink_enabled(false) });
    }

    {
        let level_str = local_sink_max_level.as_deref().unwrap_or("debug");
        let level_filter = level_to_filter(level_str);
        builder_calls.push(quote! { .with_local_sink_max_level(#level_filter) });
    }

    if install_log_capture {
        builder_calls.push(quote! { .with_install_log_capture(true) });
    }

    if !system_metrics {
        builder_calls.push(quote! { .with_system_metrics_enabled(false) });
    }

    if let Some(url) = telemetry_url {
        builder_calls.push(quote! { .with_telemetry_sink_url(#url.to_string()) });
    }

    if let Some(key) = api_key {
        builder_calls.push(quote! {
            .with_request_decorator(std::boxed::Box::new(move || std::sync::Arc::new(
                micromegas::telemetry_sink::api_key_decorator::ApiKeyRequestDecorator::new(#key.to_string())
            )))
        });
    } else {
        builder_calls.push(quote! { .with_auth_from_env() });
    }

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
            // Check CPU tracing setting before building runtime
            let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
                .map(|v| v == "true")
                .unwrap_or(false); // Default to disabled for minimal overhead

            // Set up telemetry guard BEFORE building tokio runtime
            // This ensures dispatch is initialized before worker threads start
            let _telemetry_guard = #telemetry_guard_builder;

            // Build the runtime with conditional tracing callbacks
            let runtime = {
                use micromegas::tracing::runtime::TracingRuntimeExt;
                let mut builder = tokio::runtime::Builder::new_multi_thread();
                builder.enable_all();
                builder.thread_name(env!("CARGO_PKG_NAME"));
                if cpu_tracing_enabled {
                    builder.with_tracing_callbacks();
                }
                builder.build().expect("Failed to build tokio runtime")
            };

            runtime.block_on(async move {
                // Execute the original main function body
                #original_block
            })
        }
    };

    proc_macro::TokenStream::from(expanded)
}
