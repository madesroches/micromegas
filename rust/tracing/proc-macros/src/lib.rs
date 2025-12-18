//! Procedural macros for instrumenting Rust functions with tracing spans.
//!
//! This crate provides `#[span_fn]` and `#[log_fn]` attribute macros that automatically
//! inject instrumentation into functions. These macros are the primary way to instrument
//! async code in micromegas.
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//!
//! #[span_fn]
//! async fn fetch_user(id: u64) -> User {
//!     // This async function is automatically instrumented
//!     database.get_user(id).await
//! }
//!
//! #[span_fn]
//! fn compute_hash(data: &[u8]) -> Hash {
//!     // Sync functions work too
//!     hasher.hash(data)
//! }
//! ```
//!
//! # Why use `#[span_fn]`?
//!
//! The `#[span_fn]` macro solves a fundamental challenge in async Rust: tracking execution
//! time across `.await` points. When an async function awaits, it yields control and may
//! resume on a different thread. `#[span_fn]` wraps your async code in an `InstrumentedFuture`
//! that correctly tracks wall-clock time even across these suspension points.
//!
//! For sync functions, it creates a scope-based span that measures the function's execution.
//!
//! # Import
//!
//! These macros are re-exported through the tracing prelude:
//!
//! ```rust,ignore
//! use micromegas_tracing::prelude::*;
//! ```

// crate-specific lint exceptions:
//#![allow()]

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::quote;
use syn::{
    ItemFn, ReturnType, Type, TypePath,
    parse::{Parse, ParseStream, Result},
    parse_macro_input, parse_quote,
};

struct TraceArgs {
    alternative_name: Option<Literal>,
}

impl Parse for TraceArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.is_empty() {
            Ok(Self {
                alternative_name: None,
            })
        } else {
            Ok(Self {
                alternative_name: Some(Literal::parse(input)?),
            })
        }
    }
}

/// Check if the function returns a Future (indicating it's an async trait method)
fn returns_future(function: &ItemFn) -> bool {
    match &function.sig.output {
        ReturnType::Type(_, ty) => is_future_type(ty),
        ReturnType::Default => false,
    }
}

/// Check if a type is a Future type (Pin<Box<dyn Future>> or impl Future)
fn is_future_type(ty: &Type) -> bool {
    match ty {
        // Check for Pin<Box<dyn Future<...>>>
        Type::Path(TypePath { path, .. }) => {
            if let Some(last_segment) = path.segments.last()
                && last_segment.ident == "Pin"
            {
                // This is the pattern async-trait generates
                return true;
            }
            false
        }
        // Check for impl Future<...>
        Type::ImplTrait(impl_trait) => impl_trait.bounds.iter().any(|bound| {
            if let syn::TypeParamBound::Trait(trait_bound) = bound
                && let Some(segment) = trait_bound.path.segments.last()
            {
                return segment.ident == "Future";
            }
            false
        }),
        _ => false,
    }
}

/// Instruments a function with automatic span tracing.
///
/// This is the primary macro for instrumenting both sync and async functions in micromegas.
/// It automatically detects the function type and applies the appropriate instrumentation.
///
/// # Supported Function Types
///
/// - **Sync functions**: Wrapped with `span_scope!` for scope-based timing
/// - **Async functions**: Wrapped with `InstrumentedFuture` for accurate async timing
/// - **Async trait methods**: Works with `#[async_trait]` - place `#[span_fn]` after `#[async_trait]`
///
/// # Basic Usage
///
/// ```rust,ignore
/// use micromegas_tracing::prelude::*;
///
/// // Async function - tracks time across .await points
/// #[span_fn]
/// async fn process_request(req: Request) -> Response {
///     let data = fetch_data(req.id).await;
///     transform(data).await
/// }
///
/// // Sync function - tracks wall-clock execution time
/// #[span_fn]
/// fn calculate_checksum(data: &[u8]) -> u32 {
///     data.iter().fold(0u32, |acc, &b| acc.wrapping_add(b as u32))
/// }
/// ```
///
/// # Custom Span Names
///
/// By default, the span name is the function name prefixed with the module path.
/// You can override this with a custom name:
///
/// ```rust,ignore
/// #[span_fn("custom_operation_name")]
/// async fn internal_impl() {
///     // Span will be named "module::path::custom_operation_name"
/// }
/// ```
///
/// # With Async Traits
///
/// When using `#[async_trait]`, place `#[span_fn]` on the method *after* the
/// `#[async_trait]` attribute on the impl block:
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use micromegas_tracing::prelude::*;
///
/// #[async_trait]
/// trait DataService {
///     async fn fetch(&self, id: u64) -> Data;
/// }
///
/// #[async_trait]
/// impl DataService for MyService {
///     #[span_fn]
///     async fn fetch(&self, id: u64) -> Data {
///         self.db.query(id).await
///     }
/// }
/// ```
///
/// # How It Works
///
/// For **async functions**, the macro:
/// 1. Removes the `async` keyword
/// 2. Changes the return type to `impl Future<Output = T>`
/// 3. Wraps the body in an `InstrumentedFuture` that tracks timing
///
/// For **sync functions**, the macro:
/// 1. Inserts a `span_scope!` call at the start of the function
/// 2. The span automatically closes when the function returns
///
/// # Performance
///
/// The overhead is approximately 40ns per span (20ns per event, with a span
/// recording both begin and end). This makes it suitable for high-frequency
/// instrumentation. Spans are collected in thread-local storage and batched
/// for efficient transmission.
///
/// # See Also
///
/// - [`log_fn`] - For simple function entry logging without timing
/// - `span_scope!` - For manual scope-based spans within a function
/// - `span_async_named!` - For manual async spans with dynamic names
#[proc_macro_attribute]
pub fn span_fn(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as TraceArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    let function_name = args
        .alternative_name
        .map_or(function.sig.ident.to_string(), |n| n.to_string());

    if returns_future(&function) {
        // Case 1: Async trait method (after #[async_trait] transformation)
        // Function returns Pin<Box<dyn Future<Output = T>>> and has no async keyword
        let stmts = &function.block.stmts;

        // Extract and instrument the async block from Box::pin(async move { ... })
        if stmts.len() == 1
            && let syn::Stmt::Expr(syn::Expr::Call(call_expr)) = &stmts[0]
            && call_expr.args.len() == 1
        {
            let async_block = &call_expr.args[0];

            // Replace the function body with instrumented version
            function.block = parse_quote! {
                {
                    static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                    Box::pin(InstrumentedFuture::new(
                        #async_block,
                        &_SCOPE_DESC
                    ))
                }
            };
        } else {
            // For complex async functions that don't match the simple Box::pin pattern,
            // wrap the entire body in an async block and instrument it
            let original_block = &function.block;
            function.block = parse_quote! {
                {
                    static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                    Box::pin(InstrumentedFuture::new(
                        async move #original_block,
                        &_SCOPE_DESC
                    ))
                }
            };
        }
    } else if function.sig.asyncness.is_some() {
        // Case 2: Regular async function
        let original_block = &function.block;
        let output_type = match &function.sig.output {
            syn::ReturnType::Type(_, ty) => quote! { #ty },
            syn::ReturnType::Default => quote! { () },
        };

        // Remove async and change return type to impl Future
        function.sig.asyncness = None;
        function.sig.output = parse_quote! { -> impl std::future::Future<Output = #output_type> };
        function.block = parse_quote! {
            {
                static_span_desc!(_SCOPE_DESC, concat!(module_path!(), "::", #function_name));
                let fut = async move #original_block;
                InstrumentedFuture::new(fut, &_SCOPE_DESC)
            }
        };
    } else {
        // Case 3: Regular sync function
        function.block.stmts.insert(
            0,
            parse_quote! {
                span_scope!(_METADATA_FUNC, concat!(module_path!(), "::", #function_name));
            },
        );
    }

    TokenStream::from(quote! {
        #function
    })
}

/// Logs function entry with the function name.
///
/// This macro injects a `trace!` call at the start of the function, logging
/// the function name. Unlike [`span_fn`], it does not measure execution time
/// or track function exit.
///
/// # Usage
///
/// ```rust,ignore
/// use micromegas_tracing::prelude::*;
///
/// #[log_fn]
/// fn handle_event(event: Event) {
///     // Logs "handle_event" at trace level when called
///     process(event);
/// }
/// ```
///
/// # When to Use
///
/// Use `log_fn` when you only need to know that a function was called, without
/// timing data. Note that log entries are typically more expensive than span
/// events, so for performance instrumentation prefer [`span_fn`].
///
/// `log_fn` is useful when you want function calls to appear in the log stream
/// rather than the spans/traces stream.
#[proc_macro_attribute]
pub fn log_fn(args: TokenStream, input: TokenStream) -> TokenStream {
    assert!(args.is_empty());
    let mut function = parse_macro_input!(input as ItemFn);
    let function_name = function.sig.ident.to_string();

    function.block.stmts.insert(
        0,
        parse_quote! {
            trace!(#function_name);
        },
    );
    TokenStream::from(quote! {
        #function
    })
}
