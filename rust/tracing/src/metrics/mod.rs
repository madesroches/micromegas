//! Events representing a measured scalar at a point in time
#[cfg(not(target_arch = "wasm32"))]
mod block;
#[cfg(not(target_arch = "wasm32"))]
pub use block::*;

#[cfg(target_arch = "wasm32")]
pub struct MetricsBlock;
#[cfg(target_arch = "wasm32")]
pub struct MetricsStream;

mod events;
pub use events::*;

#[cfg(not(target_arch = "wasm32"))]
mod metric_events;
#[cfg(not(target_arch = "wasm32"))]
pub use metric_events::*;
