//! transit library
//! provides fast binary serialization for Plain Old Data structures

// crate-specific lint exceptions:
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_errors_doc,
    clippy::inline_always
)]

mod dyn_string;
mod heterogeneous_queue;
mod parser;
mod reflect;
mod serialize;
mod static_string;
/// string encoding
pub mod string_codec;
/// uuid encoding
pub mod uuid_utils;
/// json-like variant
pub mod value;

pub use dyn_string::*;
pub use heterogeneous_queue::*;
pub use parser::*;
pub use reflect::*;
pub use serialize::*;
pub use static_string::*;

#[allow(unused_imports)]
#[macro_use]
extern crate micromegas_derive_transit;

pub mod prelude {
    pub use micromegas_derive_transit::*;

    pub use crate::{
        HeterogeneousQueue, InProcSerialize, InProcSize, QueueIterator, Reflect, read_any,
        write_any,
    };
}
