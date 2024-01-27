//! transit library
//! provides fast binary serialization for Plain Old Data structures

// crate-specific lint exceptions:
#![allow(unsafe_code, clippy::missing_errors_doc, clippy::inline_always)]

mod dyn_string;
mod heterogeneous_queue;
pub mod parse_string;
mod parser;
mod reflect;
mod serialize;
mod static_string;

pub use dyn_string::*;
pub use heterogeneous_queue::*;
pub use parser::*;
pub use reflect::*;
pub use serialize::*;
pub use static_string::*;

#[allow(unused_imports)]
#[macro_use]
extern crate lgn_derive_tracing_transit;

pub mod prelude {
    pub use lgn_derive_tracing_transit::*;

    pub use crate::{
        read_any, write_any, DynString, HeterogeneousQueue, InProcSerialize, InProcSize, Member,
        Object, QueueIterator, Reflect, StaticString, UserDefinedType, Value,
    };
}
