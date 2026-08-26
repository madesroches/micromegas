/**
 * Client-side color math for the histogram cell's `histogramColor` override
 * (`ColumnOverride.histogramColor`, `table-utils.tsx`) — dispatches a single
 * string value to either a named `d3-scale-chromatic` colormap (matching the
 * six names `color_scale()` supports server-side,
 * `rust/datafusion-extensions/src/color/color_scale.rs`) or a literal flat
 * CSS color.
 *
 * See `tasks/histogram_column_cell_plan.md`, Design §6.
 */

import {
  interpolateViridis,
  interpolateMagma,
  interpolatePlasma,
  interpolateInferno,
  interpolateCividis,
  interpolateTurbo,
} from 'd3-scale-chromatic'

export type ColormapName = 'viridis' | 'magma' | 'plasma' | 'inferno' | 'cividis' | 'turbo'

export const COLORMAP_NAMES: readonly ColormapName[] = [
  'viridis',
  'magma',
  'plasma',
  'inferno',
  'cividis',
  'turbo',
]

const COLORMAP_NAME_SET = new Set<string>(COLORMAP_NAMES)

const colormapInterpolators: Record<ColormapName, (t: number) => string> = {
  viridis: interpolateViridis,
  magma: interpolateMagma,
  plasma: interpolatePlasma,
  inferno: interpolateInferno,
  cividis: interpolateCividis,
  turbo: interpolateTurbo,
}

function isColormapName(color: string): color is ColormapName {
  return COLORMAP_NAME_SET.has(color)
}

/**
 * Resolves a bucket's fill color: `undefined` -> the default flat
 * `var(--chart-line)`; a recognized colormap name -> that colormap sampled
 * at `t`'s bucket-height ratio; anything else -> returned as-is (a literal
 * CSS color, flat fill, `t` unused).
 *
 * `t` is floored into `[0.5, 1]` before sampling a colormap: a zero/low-count
 * bucket must not sample the near-black end of magma/inferno-style colormaps
 * against `--app-bg` (`#0a0a0f`), or the "visible stub" guarantee (every
 * bucket's minimum-height bar must actually be visible, not just present in
 * the DOM) breaks silently. `0.5` targets roughly 3:1 contrast against
 * `--app-bg` for the darkest colormaps (magma, inferno) — a shallower floor
 * like `0.15` measures only ~1.1-1.23:1 contrast there and isn't actually
 * visible.
 */
export function resolveHistogramBarColor(color: string | undefined, t: number): string {
  if (!color) return 'var(--chart-line)'
  if (isColormapName(color)) {
    return colormapInterpolators[color](0.5 + t * 0.5)
  }
  return color
}

/**
 * Builds a CSS `linear-gradient(...)` string previewing a colormap end to
 * end, for the Overrides panel's swatch picker (Design §6's Editor UI).
 * Samples the raw `t = 0..1` range (not `resolveHistogramBarColor`'s floored
 * `[0.5, 1]`) — the swatch shows "this is the viridis colormap", not a
 * specific bucket's exact rendered color — from the same interpolator
 * `resolveHistogramBarColor` uses, so the preview can never drift from what
 * clicking it actually produces.
 */
export function buildColormapPreviewGradient(name: ColormapName): string {
  const stops = [0, 0.2, 0.4, 0.6, 0.8, 1].map((t) => colormapInterpolators[name](t))
  return `linear-gradient(to right, ${stops.join(', ')})`
}
