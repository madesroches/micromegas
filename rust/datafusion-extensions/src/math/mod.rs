//! Scalar math UDFs for 1D remapping and related helpers.
//!
//! # API conventions
//!
//! These conventions apply to every math UDF in this crate. Once external
//! SQL exists in the wild they are effectively frozen — any new math UDF
//! must honour them, and behaviour-changing variants must adopt a distinct
//! name.
//!
//! - **No clamping.** `lerp(a, b, t)` with `t` outside `[0, 1]` extrapolates;
//!   `unlerp(a, b, x)` with `x` outside `[a, b]` returns a value outside
//!   `[0, 1]`. Callers who want clamping wrap with
//!   `LEAST(GREATEST(t, 0.0), 1.0)`.
//! - **IEEE-754 propagation, not errors.** `NaN`/`±∞` inputs propagate via
//!   float math. Degenerate inputs (e.g. `unlerp(a, a, x)`) produce `NaN`
//!   or `±Inf` rather than per-row errors. Callers who want a fallback
//!   wrap with the existing `nanvl(...)` built-in.
//! - **Nulls propagate.** If any input is `NULL`, the row's result is
//!   `NULL`. Matches every other scalar UDF in this crate.
//! - **Float64 only.** All signatures use `Signature::exact(vec![Float64; N], …)`.
//!   DataFusion's implicit numeric coercion lets callers write integer
//!   literals (`lerp(0, 1, 0.5)`) without explicit casts.
//!
//! Future-extension naming reservations (recorded so the API stays coherent
//! as it grows):
//! - `smoothstep(edge0, edge1, x) -> Float64` — HLSL/GLSL-style smooth
//!   Hermite interpolation. Composes with `lerp` the same way `unlerp` does.
//! - `map_range(x, a, b, c, d) -> Float64` — one-shot remap. Currently
//!   superseded by `lerp(c, d, unlerp(a, b, x))`; name reserved.
//! - `saturate(x) -> Float64` — HLSL-style `clamp(x, 0, 1)`. Sibling to
//!   wrap around `unlerp` results when callers want `[0, 1]`-clamped output.

/// `lerp(a, b, t) -> Float64`
pub mod lerp;
/// `unlerp(a, b, x) -> Float64`
pub mod unlerp;
