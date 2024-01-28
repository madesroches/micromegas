pub mod encoding;
mod errors;

mod bytes;

pub use errors::{Error, Result};

pub use self::bytes::Bytes;

#[macro_export]
macro_rules! include_api {
    () => {
        include!(concat!(env!("OUT_DIR"), "/api.rs"));
    };
}
