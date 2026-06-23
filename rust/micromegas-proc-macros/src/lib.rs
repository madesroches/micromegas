//! Top-level procedural macros for micromegas
//!
//! This crate provides high-level procedural macros that integrate multiple
//! micromegas components for a seamless developer experience.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{ItemFn, Lit, Meta, NestedMeta};

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
    expand_micromegas_main(args.into(), input.into())
        .unwrap_or_else(|err| err.to_compile_error())
        .into()
}

fn expand_micromegas_main(
    args: TokenStream,
    input: TokenStream,
) -> Result<TokenStream, syn::Error> {
    use syn::parse::Parser;

    let args: Vec<NestedMeta> =
        syn::punctuated::Punctuated::<NestedMeta, syn::Token![,]>::parse_terminated
            .parse2(args)?
            .into_iter()
            .collect();

    let function: ItemFn = syn::parse2(input)?;

    if function.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            function.sig.fn_token,
            "micromegas_main can only be applied to async functions",
        ));
    }

    if function.sig.ident != "main" {
        return Err(syn::Error::new_spanned(
            &function.sig.ident,
            "micromegas_main can only be applied to the main function",
        ));
    }

    let mut interop_max_level: Option<syn::LitStr> = None;
    let mut max_level_override: Option<syn::LitStr> = None;
    let mut ctrlc_handling: bool = true;
    let mut local_sink_enabled: bool = true;
    let mut local_sink_max_level: Option<syn::LitStr> = None;
    let mut install_log_capture: bool = false;
    let mut system_metrics: bool = true;
    let mut telemetry_url: Option<String> = None;
    let mut api_key: Option<String> = None;

    for arg in args {
        match arg {
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("interop_max_level") => {
                if let Lit::Str(lit_str) = &nv.lit {
                    interop_max_level = Some(lit_str.clone());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "interop_max_level must be a string literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("max_level_override") => {
                if let Lit::Str(lit_str) = &nv.lit {
                    max_level_override = Some(lit_str.clone());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "max_level_override must be a string literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("ctrlc_handling") => {
                if let Lit::Bool(lit_bool) = &nv.lit {
                    ctrlc_handling = lit_bool.value();
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "ctrlc_handling must be a bool literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("local_sink_enabled") => {
                if let Lit::Bool(lit_bool) = &nv.lit {
                    local_sink_enabled = lit_bool.value();
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "local_sink_enabled must be a bool literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("local_sink_max_level") => {
                if let Lit::Str(lit_str) = &nv.lit {
                    local_sink_max_level = Some(lit_str.clone());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "local_sink_max_level must be a string literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("install_log_capture") => {
                if let Lit::Bool(lit_bool) = &nv.lit {
                    install_log_capture = lit_bool.value();
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "install_log_capture must be a bool literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("system_metrics") => {
                if let Lit::Bool(lit_bool) = &nv.lit {
                    system_metrics = lit_bool.value();
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "system_metrics must be a bool literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("telemetry_url") => {
                if let Lit::Str(lit_str) = &nv.lit {
                    telemetry_url = Some(lit_str.value());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "telemetry_url must be a string literal",
                    ));
                }
            }
            NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("api_key") => {
                if let Lit::Str(lit_str) = &nv.lit {
                    api_key = Some(lit_str.value());
                } else {
                    return Err(syn::Error::new_spanned(
                        &nv.lit,
                        "api_key must be a string literal",
                    ));
                }
            }
            other => {
                return Err(syn::Error::new_spanned(
                    &other,
                    "Unsupported attribute argument. Supported: api_key, ctrlc_handling, install_log_capture, interop_max_level, local_sink_enabled, local_sink_max_level, max_level_override, system_metrics, telemetry_url",
                ));
            }
        }
    }

    let original_block = &function.block;
    let return_type = &function.sig.output;

    let level_to_filter = |lit: &syn::LitStr| -> Result<TokenStream, syn::Error> {
        Ok(match lit.value().to_lowercase().as_str() {
            "trace" => quote! { micromegas::tracing::levels::LevelFilter::Trace },
            "debug" => quote! { micromegas::tracing::levels::LevelFilter::Debug },
            "info" => quote! { micromegas::tracing::levels::LevelFilter::Info },
            "warn" => quote! { micromegas::tracing::levels::LevelFilter::Warn },
            "error" => quote! { micromegas::tracing::levels::LevelFilter::Error },
            "off" => quote! { micromegas::tracing::levels::LevelFilter::Off },
            _ => {
                return Err(syn::Error::new_spanned(
                    lit,
                    "Invalid level value. Must be one of: trace, debug, info, warn, error, off",
                ));
            }
        })
    };

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
        let level_filter = match &local_sink_max_level {
            Some(lit) => level_to_filter(lit)?,
            None => quote! { micromegas::tracing::levels::LevelFilter::Debug },
        };
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

    if let Some(lit) = &max_level_override {
        let level_filter = level_to_filter(lit)?;
        builder_calls.push(quote! { .with_max_level_override(#level_filter) });
    }

    if let Some(lit) = &interop_max_level {
        let level_filter = level_to_filter(lit)?;
        builder_calls.push(quote! { .with_interop_max_level_override(#level_filter) });
    }

    let telemetry_guard_builder = quote! {
        micromegas::telemetry_sink::TelemetryGuardBuilder::default()
            #(#builder_calls)*
            .build()
    };

    Ok(quote! {
        fn main() #return_type {
            let cpu_tracing_enabled = std::env::var("MICROMEGAS_ENABLE_CPU_TRACING")
                .map(|v| v == "true")
                .unwrap_or(false);

            let _telemetry_guard = #telemetry_guard_builder;

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
                #original_block
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::expand_micromegas_main;
    use quote::quote;

    fn expand(args: proc_macro2::TokenStream) -> String {
        let input = quote! { async fn main() {} };
        expand_micromegas_main(args, input)
            .expect("expansion should succeed")
            .to_string()
    }

    #[test]
    fn default_produces_standard_calls() {
        let out = expand(quote! {});
        assert!(out.contains("with_auth_from_env"));
        assert!(out.contains("with_ctrlc_handling"));
        assert!(out.contains("with_local_sink_max_level"));
    }

    #[test]
    fn api_key_replaces_env_auth() {
        let out = expand(quote! { api_key = "secret" });
        assert!(out.contains("ApiKeyRequestDecorator"));
        assert!(!out.contains("with_auth_from_env"));
    }

    #[test]
    fn ctrlc_handling_false_omits_call() {
        let out = expand(quote! { ctrlc_handling = false });
        assert!(!out.contains("with_ctrlc_handling"));
    }

    #[test]
    fn telemetry_url_emits_call() {
        let out = expand(quote! { telemetry_url = "http://localhost:9000" });
        assert!(out.contains("with_telemetry_sink_url"));
    }

    #[test]
    fn local_sink_disabled_emits_call() {
        let out = expand(quote! { local_sink_enabled = false });
        assert!(out.contains("with_local_sink_enabled"));
    }

    #[test]
    fn system_metrics_false_emits_call() {
        let out = expand(quote! { system_metrics = false });
        assert!(out.contains("with_system_metrics_enabled"));
    }

    #[test]
    fn install_log_capture_true_emits_call() {
        let out = expand(quote! { install_log_capture = true });
        assert!(out.contains("with_install_log_capture"));
    }

    #[test]
    fn local_sink_max_level_custom_emits_correct_filter() {
        let out = expand(quote! { local_sink_max_level = "info" });
        assert!(out.contains("LevelFilter :: Info"));
    }

    fn expand_err(args: proc_macro2::TokenStream) -> syn::Error {
        let input = quote! { async fn main() {} };
        expand_micromegas_main(args, input).expect_err("expansion should fail")
    }

    #[test]
    fn bad_ctrlc_type_is_error() {
        let err = expand_err(quote! { ctrlc_handling = "not_a_bool" });
        assert_eq!(err.to_string(), "ctrlc_handling must be a bool literal");
    }

    #[test]
    fn unknown_arg_is_error() {
        let err = expand_err(quote! { unknown_arg = true });
        assert!(err.to_string().contains("Unsupported attribute argument"));
    }

    #[test]
    fn invalid_level_is_error() {
        let err = expand_err(quote! { max_level_override = "verbose" });
        assert!(err.to_string().contains("Invalid level value"));
    }

    #[test]
    fn non_async_fn_is_error() {
        let err = expand_micromegas_main(quote! {}, quote! { fn main() {} })
            .expect_err("non-async main should fail");
        assert!(err.to_string().contains("async functions"));
    }

    #[test]
    fn malformed_args_is_error() {
        // Garbage tokens that are not valid attribute meta items.
        let err = expand_micromegas_main(quote! { = = = }, quote! { async fn main() {} })
            .expect_err("malformed args should fail");
        // A parse error carries a span-anchored message rather than a panic.
        assert!(!err.to_string().is_empty());
    }
}
