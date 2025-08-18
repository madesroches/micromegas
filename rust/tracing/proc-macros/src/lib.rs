//! `log_fn` and `span_fn` procedural macros
//!
//! Injects instrumentation into sync and async functions.
//! `span_fn` supports sync functions, async functions, and async trait methods automatically.

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


/// span_fn: trace the execution of sync functions, async functions, and async trait methods
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
        if stmts.len() == 1 {
            if let syn::Stmt::Expr(syn::Expr::Call(call_expr)) = &stmts[0] {
                if call_expr.args.len() == 1 {
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
                }
            }
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
