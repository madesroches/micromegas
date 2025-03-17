//! `log_fn` and `span_fn` procedural macros
//!
//! Injects instrumentation into sync and async functions.
//!     async trait functions not supported

// crate-specific lint exceptions:
//#![allow()]

use proc_macro2::Literal;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream, Result},
    parse_macro_input, parse_quote, ItemFn,
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

/// span_fn: trace the execution of a function
#[proc_macro_attribute]
pub fn span_fn(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut function = parse_macro_input!(input as ItemFn);

    if function.sig.asyncness.is_some() {
        // NOOP For now
        return proc_macro::TokenStream::from(quote! {
            #function
        });
    };

    let args = parse_macro_input!(args as TraceArgs);

    let function_name = args
        .alternative_name
        .map_or(function.sig.ident.to_string(), |n| n.to_string());

    function.block.stmts.insert(
        0,
        parse_quote! {
            micromegas_tracing::span_scope!(_METADATA_FUNC, concat!(module_path!(), "::", #function_name));
        },
    );

    proc_macro::TokenStream::from(quote! {
        #function
    })
}

/// log_fn: log the execution of a function
#[proc_macro_attribute]
pub fn log_fn(
    args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    assert!(args.is_empty());
    let mut function = parse_macro_input!(input as ItemFn);
    let function_name = function.sig.ident.to_string();

    function.block.stmts.insert(
        0,
        parse_quote! {
            micromegas_tracing::trace!(#function_name);
        },
    );
    proc_macro::TokenStream::from(quote! {
        #function
    })
}
