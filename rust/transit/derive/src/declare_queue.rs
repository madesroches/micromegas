use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse, DeriveInput, GenericParam};

fn gen_read_method(
    type_args: &[syn::Ident],
    any_ident: &syn::Ident,
) -> quote::__private::TokenStream {
    let mut value_tupe_counter: u8 = 0;
    let type_index_cases = type_args.iter().map(|value_type_id| {
        let index = value_tupe_counter;
        value_tupe_counter += 1;
        quote! {
            #index => {
                unsafe {
                    let mut begin_obj = self.buffer.as_ptr().add( offset+1 );
                    let next_object_offset;
                    let value_size = if let InProcSize::Const(size) = <#value_type_id as micromegas_transit::InProcSerialize>::IN_PROC_SIZE {
                        next_object_offset = offset + 1 + size;
                        None
                    } else {
                        let size_instance = micromegas_transit::read_any::<u32>(begin_obj);
                        begin_obj = begin_obj.add( std::mem::size_of::<u32>() );
                        next_object_offset = offset + 1 + std::mem::size_of::<u32>() + size_instance as usize;
                        Some(size_instance)
                    };
                    let obj = #any_ident::#value_type_id( <#value_type_id as micromegas_transit::InProcSerialize>::read_value(begin_obj, value_size) );
                    (obj,next_object_offset)
                }
            },
        }
    });

    quote! {
        fn read_value_at_offset( &self, offset: usize ) -> (#any_ident, usize){
            let index = self.buffer[offset];
            match index{
                #(#type_index_cases)*
                _ => {
                    panic!("unknown type index");
                }
            }
        }
    }
}

fn gen_type_index_impls(
    type_args: &[syn::Ident],
    type_index_ident: &syn::Ident,
) -> quote::__private::TokenStream {
    let mut value_tupe_counter: u8 = 0;
    let type_index_impls = type_args.iter().map(|value_type_id| {
        let index = value_tupe_counter;
        value_tupe_counter += 1;
        quote! {
            impl #type_index_ident for #value_type_id {
                const TYPE_INDEX: u8 = #index;
            }
        }
    });

    quote! {
        #(#type_index_impls)*
    }
}

fn gen_hetero_queue_impl(
    struct_identifier: &syn::Ident,
    type_args: &[syn::Ident],
    any_ident: &syn::Ident,
) -> quote::__private::TokenStream {
    let read_method = gen_read_method(type_args, any_ident);
    quote! {
        impl micromegas_transit::HeterogeneousQueue for #struct_identifier {
            type Item = #any_ident;

            fn new(buffer_size: usize) -> Self {
                Self { buffer: Vec::with_capacity(buffer_size),
                       obj_counter: 0,
                }
            }

            fn reflect_contained() -> Vec<UserDefinedType> {
                vec![ #(#type_args::reflect(),)* ]
            }


            fn len_bytes(&self) -> usize{
                self.buffer.len()
            }

            fn nb_objects(&self) -> usize{
                self.obj_counter
            }

            fn capacity_bytes(&self) -> usize{
                self.buffer.capacity()
            }

            fn iter(&self) -> QueueIterator<'_, Self, #any_ident> {
                QueueIterator::begin(self)
            }

            #read_method

        }
    }
}

pub fn declare_queue_impl(input: TokenStream) -> TokenStream {
    let ast = parse::<DeriveInput>(input).unwrap();
    let struct_identifier = ast.ident.clone();
    let struct_name_str = struct_identifier.to_string();

    let type_args: Vec<syn::Ident> = ast
        .generics
        .params
        .iter()
        .map(|p| match p {
            GenericParam::Type(t) => t.ident.clone(),
            GenericParam::Lifetime(_) => panic!("lifetime of generic param not supported"),
            GenericParam::Const(_) => panic!("const generic param not supported"),
        })
        .collect();

    let any_ident = format_ident!("{}Any", struct_identifier);
    let type_index_ident = format_ident!("{}TypeIndex", struct_identifier);
    let type_index_impls = gen_type_index_impls(&type_args, &type_index_ident);
    let reflective_queue_impl = gen_hetero_queue_impl(&struct_identifier, &type_args, &any_ident);

    TokenStream::from(quote! {

        #[derive(Debug)]
        #[allow(clippy::enum_variant_names)]
        pub enum #any_ident{
            #(#type_args(#type_args),)*
        }

        pub struct #struct_identifier {
            buffer: Vec<u8>,
            obj_counter: usize,
        }

        #reflective_queue_impl

        impl std::fmt::Debug for #struct_identifier{
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error>{
                f.debug_struct(#struct_name_str)
                    .finish()
            }
        }

        impl #struct_identifier {
            #[inline(always)]
            pub fn as_bytes(&self) -> &[u8]{
                &self.buffer
            }

            #[inline(always)]
            pub fn into_bytes(self) -> Vec<u8>{
                self.buffer
            }

            #[inline]
            pub fn push<T>(&mut self, value: T)
            where
                T: micromegas_transit::InProcSerialize + #type_index_ident,
            {
                self.obj_counter += 1;

                // write type discriminant
                self.buffer.push(<T as #type_index_ident>::TYPE_INDEX);

                let buffer_size_before = self.buffer.len();
                if let InProcSize::Const(size) = T::IN_PROC_SIZE {
                    value.write_value(&mut self.buffer);
                    assert!(self.buffer.len() == buffer_size_before + size);
                } else {
                    // we force the dynamically sized object to first serialize their size as unsigned 32 bits
                    // this will allow unparsable objects to be skipped by the reader
                    let value_size = T::get_value_size(&value).unwrap();
                    micromegas_transit::write_any(&mut self.buffer, &value_size);
                    value.write_value(&mut self.buffer);
                    assert!(
                        self.buffer.len()
                            == buffer_size_before + std::mem::size_of::<u32>() + value_size as usize
                    );
                }
            }
        }

        pub trait #type_index_ident {
            const TYPE_INDEX: u8;
        }

        #type_index_impls

    })
}
