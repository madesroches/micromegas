//! Color UDFs for building packed RGBA `u32` colors from SQL.
//!
//! # API conventions
//!
//! These conventions apply to every color UDF in this crate. Once external SQL
//! exists in the wild they are effectively frozen — any new color UDF must
//! honour them, and behaviour-changing variants must adopt a distinct name.
//!
//! - **Packing.** Colors are packed `u32` in `0xRRGGBBAA` byte order: byte 0
//!   (high byte) is red, byte 3 (low byte) is alpha. This matches the map
//!   cell's decode in `MapViewer.tsx`.
//! - **Component range.** Float inputs/outputs are in `[0.0, 1.0]`.
//!   Out-of-range values are clamped at the byte boundary, not rejected.
//! - **Alpha is straight, not premultiplied.** The alpha channel is treated
//!   the same as RGB — it interpolates and quantizes the same way.
//! - **Color space is sRGB-encoded 8-bit.** Lerps and other operations act
//!   directly on sRGB byte values, which is what the GPU consumes and what
//!   HLSL/Cg-style 8-bit color code does. Future perceptual-space variants
//!   (e.g. `lerp_oklab`) must use an explicit suffix; unsuffixed names always
//!   mean sRGB.
//!
//! Future-extension naming reservations (recorded so the API stays coherent
//! as it grows):
//! - Constructors by format: `rgba` (this module), `rgb`, `hsla`, `hsva`,
//!   `color_from_hex`.
//! - Operations on packed colors: `<op>_color` suffix — `lerp_color` (this
//!   module), and (future) `mix_color`, `blend_color`, `tint_color`,
//!   `saturate_color`.
//! - Color-space-specific operations: `<op>_<space>` suffix — `lerp_oklab`,
//!   `lerp_hsl`, etc.

/// `lerp_color(c1, c2, t) -> UInt32`
pub mod lerp_color;
/// `rgba(r, g, b, a) -> UInt32`
pub mod rgba;

/// Pack four 8-bit channels into a `0xRRGGBBAA` `u32`.
#[inline]
pub fn pack_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)
}

/// Unpack a `0xRRGGBBAA` `u32` into `(r, g, b, a)`.
#[inline]
pub fn unpack_rgba(c: u32) -> (u8, u8, u8, u8) {
    (
        ((c >> 24) & 0xff) as u8,
        ((c >> 16) & 0xff) as u8,
        ((c >> 8) & 0xff) as u8,
        (c & 0xff) as u8,
    )
}

/// Quantize a normalized float to `0..=255` with round-half-up.
///
/// Clamps the scaled value (not the input) so the output invariant holds
/// regardless of input range. ±∞ saturate via the clamp; NaN saturates to 0
/// via Rust's saturating `f64 as u8` cast (`f64::clamp` propagates NaN, so
/// the cast is what makes NaN inputs safe). Both safety nets are load-bearing
/// — do not remove either one.
#[inline]
pub fn float_to_byte(f: f64) -> u8 {
    (f * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Round an already-in-`[0,255]` lerp result to a `u8`, half-up.
///
/// Shared with `lerp_color` so both UDFs use the same tie-breaking rule.
#[inline]
pub fn round_to_byte(f: f64) -> u8 {
    (f + 0.5).clamp(0.0, 255.0) as u8
}
