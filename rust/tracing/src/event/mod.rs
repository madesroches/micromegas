//! Structure to record events in memory
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(not(target_arch = "wasm32"))]
pub mod in_memory_sink;

mod sink;
pub use sink::*;

#[cfg(not(target_arch = "wasm32"))]
mod stream;
#[cfg(not(target_arch = "wasm32"))]
pub use stream::*;
