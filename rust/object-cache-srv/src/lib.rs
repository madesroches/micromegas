//! Shared object range cache service library.
//!
//! Split out from the `micromegas-object-cache-srv` binary so integration
//! tests (which compile as a separate crate under `tests/`) can exercise the
//! handlers and app state directly.

pub mod app_state;
pub mod cli;
pub mod handlers;
pub mod prefetch_queue;
pub mod saturation_monitor;
pub mod validation;
