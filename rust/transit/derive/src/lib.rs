//! derive-transit library
//! provides procedural macros like #[derive(Reflect)] to allow reflection and
//! fast serialization

// crate-specific lint exceptions:
//#![allow()]

mod declare_queue;
mod derive_reflect;
use proc_macro::TokenStream;

/// generate reflection code
///
/// `#[transit(reference)]` on a field marks it as a reference, for a type that is not
/// syntactically a pointer but holds a dependency id.
#[proc_macro_derive(TransitReflect, attributes(transit))]
pub fn derive_reflect(input: TokenStream) -> TokenStream {
    derive_reflect::derive_reflect_impl(input)
}

/// generate heterogenous queue
#[proc_macro]
pub fn declare_queue_struct(input: TokenStream) -> TokenStream {
    declare_queue::declare_queue_impl(input)
}
