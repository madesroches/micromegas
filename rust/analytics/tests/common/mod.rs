//! Shared helpers for DB-backed integration tests under `tests/`. Not a test binary itself:
//! cargo only compiles top-level files directly under `tests/` as separate test binaries, so this
//! subdirectory (reached via `mod common;` from each test file that needs it) is the standard way
//! to share code between them without tripping that per-file-is-a-binary rule.

pub mod db_fixtures;
