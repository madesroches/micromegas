//! Events representing a process's log
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct LogBlock;
#[cfg(target_arch = "wasm32")]
pub struct LogStream;

mod events;
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod log_events;
#[cfg(not(target_arch = "wasm32"))]
pub use log_events::*;
