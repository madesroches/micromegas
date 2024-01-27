use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput};
type QuoteRes = quote::__private::TokenStream;

fn metadata_from_type(t: &syn::Type) -> (QuoteRes, bool) {
    match t {
        syn::Type::Array(_) => panic!("Array field type not supported"),
        syn::Type::BareFn(fun) => (quote! {#fun}, true),
        syn::Type::Group(_) => panic!("Group field type not supported"),
        syn::Type::ImplTrait(_) => panic!("ImplTrait field type not supported"),
        syn::Type::Infer(_) => panic!("Infer field type not supported"),
        syn::Type::Macro(_) => panic!("Macro field type not supported"),
        syn::Type::Never(_) => panic!("Never field type not supported"),
        syn::Type::Paren(_) => panic!("Paren field type not supported"),
        syn::Type::Path(type_path) => (quote! {#type_path}, false),
        syn::Type::Ptr(pointer_type) => (quote! {#pointer_type}, true),
        syn::Type::Reference(reference) => (quote! {#reference}, true),
        syn::Type::Slice(_) => panic!("Slice field type not supported"),
        syn::Type::TraitObject(_) => panic!("TraitObject field type not supported"),
        syn::Type::Tuple(_) => panic!("Tuple field type not supported"),
        syn::Type::Verbatim(_) => panic!("Verbatim field type not supported"),
        unknown_field_type => {
            panic!("Unexpected field type: {:?}", unknown_field_type)
        }
    }
}

pub fn derive_reflect_impl(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let udt_identifier = ast.ident.clone();
    let udt_name = ast.ident.to_string();
    let mut members = Vec::new();

    match ast.data {
        syn::Data::Struct(s) => match s.fields {
            syn::Fields::Named(named_fields) => {
                for field in named_fields.named {
                    let field_name = field.ident.unwrap().to_string();
                    let (field_type, is_reference) = metadata_from_type(&field.ty);
                    members.push((field_name, field_type, is_reference));
                }
            }
            syn::Fields::Unnamed(_) => panic!("only named fields are supported"),
            syn::Fields::Unit => panic!("unit fields not expected"),
        },
        syn::Data::Enum(_) => panic!("enums not supported"),
        syn::Data::Union(_) => panic!("bunions not supported"),
    }

    let mut secondary_types = vec![];
    for m in &members {
        let member_type = &m.1;
        let is_reference = &m.2;
        let type_name = member_type.to_string();
        if *is_reference
            || type_name == "i64"
            || type_name == "u64"
            || type_name == "u32"
            || type_name == "i32"
            || type_name == "f64"
        {
            continue;
        }
        secondary_types.push(member_type.clone());
    }

    let secondary_types_toks = secondary_types.iter().map(|t| {
        quote! {
            #t::reflect(),
        }
    });

    let members_toks = members.iter().map(|m| {
        let member_name = &m.0;
        let member_ident = format_ident!("{}", &m.0);
        let member_type = &m.1;
        let member_type_name = member_type.to_string();
        let is_reference = &m.2;
        quote! {
            Member{ name: String::from(#member_name),
                    type_name: String::from(#member_type_name),
                    offset: memoffset::offset_of!(#udt_identifier,#member_ident),
                    size: std::mem::size_of::<#member_type>(),
                    is_reference: #is_reference,
        },
        }
    });

    TokenStream::from(quote! {
        impl lgn_tracing_transit::Reflect for #udt_identifier{
            fn reflect() -> UserDefinedType{
                UserDefinedType{
                    name: String::from(#udt_name),
                    size: std::mem::size_of::<#udt_identifier>(),
                    members: vec![#(#members_toks)*],
                    is_reference: false,
                    secondary_udts: vec![#(#secondary_types_toks)*],
                }
            }
        }
    })
}
