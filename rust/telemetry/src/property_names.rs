//! Property-key constants shared by producers and the ingestion stack.
//!
//! Kept apart from the `property` module, which is gated behind the `server` feature because its
//! `Property` type carries sqlx impls: client crates need these names to honor the reserved
//! namespace before they send anything, and must not pull a database driver in to do it.

/// Reserved, server-written property namespace. A client-supplied property whose key starts
/// with this prefix is dropped at ingestion.
pub const RESERVED_PROPERTY_PREFIX: &str = "micromegas.";
