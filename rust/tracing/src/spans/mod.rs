//! Events reprensenting units of code execution
mod block;
pub use block::*;

mod events;
pub use events::*;

mod instrumented_future;
pub use instrumented_future::*;

// todo: implement non thread based perf spans for other systems to be used
