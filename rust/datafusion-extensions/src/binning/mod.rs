//! Binning UDFs for snapping continuous coordinates to discrete grids.
//!
//! # API conventions
//!
//! These conventions apply to every binning UDF in this crate. Once external
//! SQL exists in the wild they are effectively frozen — any new binning UDF
//! must honour them, and behaviour-changing variants must adopt a distinct
//! name.
//!
//! - **Centered on zero.** `bin_center(0, cs) = 0`. The bin containing
//!   `coord` spans the half-open interval `[c - cs/2, c + cs/2)` where
//!   `c` is the returned center.
//! - **Half-open intervals.** A point that lands exactly on a bin edge
//!   (`coord = c + cs/2`) belongs to the *next* bin. This matches the standard
//!   `FLOOR` convention used by the `histogram` module's bucketing.
//! - **Pathological inputs are not validated.** `cell_size <= 0` and
//!   `NaN`/`±∞` inputs are not rejected. Float math propagates `NaN`/`Inf`
//!   naturally; a zero or negative `cell_size` produces undefined results.
//!   Documented as a precondition.
//!
//! Future-extension naming reservations (recorded so the API stays coherent
//! as it grows):
//! - `bin_index(coord, cs) -> Int64` — the raw `floor(...)` result, for
//!   sparse-key use cases.
//! - Vectorized 2D/N-D variants are deliberately not offered; the 1D form
//!   composes trivially (`bin_center(x, cs), bin_center(y, cs)`).

/// `bin_center(coord, cell_size) -> Float64`
pub mod bin_center;
