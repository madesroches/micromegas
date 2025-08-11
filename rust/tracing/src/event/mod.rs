//! Structure to record events in memory
mod block;
pub use block::*;

pub mod in_memory_sink;

mod sink;
pub use sink::*;

mod stream;
pub use stream::*;
