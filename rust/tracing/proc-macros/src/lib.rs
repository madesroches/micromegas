//! `log_fn` and `span_fn` procedural macros
//!
//! Injects instrumentation into sync and async functions.
//! `span_fn` now supports both sync and async functions automatically.
//!     async trait functions not supported

// crate-specific lint exceptions:
//#![allow()]

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::quote;
use syn::{
    ItemFn, Type, TypeImplTrait, TypePath,
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

/// span_fn: trace the execution of a sync or async function
#[proc_macro_attribute]
pub fn span_fn(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as TraceArgs);
    let mut function = parse_macro_input!(input as ItemFn);

    let function_name = args
        .alternative_name
        .map_or(function.sig.ident.to_string(), |n| n.to_string());

    if function.sig.asyncness.is_some() {
        // Handle async functions using InstrumentedFuture approach
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
        // Handle sync functions
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

/// log_fn: log the execution of a function
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
