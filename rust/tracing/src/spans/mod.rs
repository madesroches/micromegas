//! Events reprensenting units of code execution
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct ThreadBlock;
#[cfg(target_arch = "wasm32")]
pub struct ThreadStream;

mod events;
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod span_events;
#[cfg(not(target_arch = "wasm32"))]
pub use span_events::*;

mod instrumented_future;
pub use instrumented_future::*;

// todo: implement non thread based perf spans for other systems to be used
