# Micromegas Extended Color Palette

Inspired by Van Gogh's "Wheatfield with Crows" (1890) - turbulent skies, golden wheat, and earthy paths.

## Primary Colors (existing)

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Rust Orange | `#bf360c` | 191, 54, 12 | Primary accent, ring 1 |
| Cobalt Blue | `#1565c0` | 21, 101, 192 | Secondary accent, ring 2 |
| Wheat | `#ffb300` | 255, 179, 0 | Tertiary accent, ring 3 |
| Deep Night | `#0a0a0f` | 10, 10, 15 | Dark background |
| Charcoal | `#1a1a2e` | 26, 26, 46 | Dark text/icons |

## Extended Palette - Stormy Sky (blues/purples)

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Storm Blue | `#1a237e` | 26, 35, 126 | Deep accent, headers |
| Twilight | `#283593` | 40, 53, 147 | Charts, secondary data |
| Prussian | `#0d47a1` | 13, 71, 161 | Links, interactive |
| Horizon | `#42a5f5` | 66, 165, 245 | Info states, highlights |
| Violet Dusk | `#5e35b1` | 94, 53, 177 | Tertiary data series |
| Lavender Storm | `#7e57c2` | 126, 87, 194 | Light accent, tags |

## Extended Palette - Wheat Field (yellows/golds)

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Harvest Gold | `#ff8f00` | 255, 143, 0 | Warnings, emphasis |
| Amber | `#ffc107` | 255, 193, 7 | Secondary gold |
| Ripe Grain | `#ffd54f` | 255, 213, 79 | Highlights, hover |
| Pale Straw | `#ffecb3` | 255, 236, 179 | Light backgrounds |
| Ochre | `#e65100` | 230, 81, 0 | Critical warnings |
| Burnt Sienna | `#8d3a14` | 141, 58, 20 | Dark rust accent |

## Extended Palette - Earth & Path (browns/greens)

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Field Green | `#2e7d32` | 46, 125, 50 | Success states |
| Sage | `#66bb6a` | 102, 187, 106 | Positive data |
| Olive Path | `#827717` | 130, 119, 23 | Muted accent |
| Umber | `#4e342e` | 78, 52, 46 | Dark earth accent |
| Clay | `#6d4c41` | 109, 76, 65 | Warm neutral |
| Tilled Earth | `#3e2723` | 62, 39, 35 | Deep brown |

## Extended Palette - Crow & Shadow (neutrals)

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Crow Black | `#121212` | 18, 18, 18 | True black accents |
| Shadow | `#1e1e2f` | 30, 30, 47 | Card backgrounds |
| Slate | `#37474f` | 55, 71, 79 | Secondary text |
| Pewter | `#546e7a` | 84, 110, 122 | Muted text |
| Cloud Grey | `#90a4ae` | 144, 164, 174 | Disabled states |
| Misty | `#cfd8dc` | 207, 216, 220 | Borders, dividers |

## Extended Palette - Accent & Status

| Name | Hex | RGB | Use |
|------|-----|-----|-----|
| Crimson | `#c62828` | 198, 40, 40 | Errors, critical |
| Coral | `#ff7043` | 255, 112, 67 | Attention needed |
| Teal | `#00897b` | 0, 137, 123 | Alternative success |
| Cyan | `#00acc1` | 0, 172, 193 | Info, links |
| Pink Dusk | `#ad1457` | 173, 20, 87 | Accent, special |
| Lime | `#9e9d24` | 158, 157, 36 | Neutral positive |

## Chart Color Sequences

### Primary Sequence (12 colors for data series)
```
#bf360c  Rust Orange
#1565c0  Cobalt Blue
#ffb300  Wheat
#2e7d32  Field Green
#5e35b1  Violet Dusk
#ff8f00  Harvest Gold
#00897b  Teal
#c62828  Crimson
#7e57c2  Lavender Storm
#827717  Olive Path
#00acc1  Cyan
#ad1457  Pink Dusk
```

### Sequential Blues (for gradients/heatmaps)
```
#e3f2fd → #90caf9 → #42a5f5 → #1565c0 → #0d47a1 → #1a237e
```

### Sequential Oranges (for gradients/heatmaps)
```
#fff3e0 → #ffcc80 → #ff8f00 → #bf360c → #8d3a14 → #4e342e
```

### Diverging (for comparison data)
```
#bf360c → #ff8f00 → #ffd54f → #f5f5f7 → #90caf9 → #1565c0 → #1a237e
```

## CSS Variables

```css
:root {
  /* Primary */
  --color-rust: #bf360c;
  --color-cobalt: #1565c0;
  --color-wheat: #ffb300;
  --color-night: #0a0a0f;
  --color-charcoal: #1a1a2e;

  /* Stormy Sky */
  --color-storm: #1a237e;
  --color-twilight: #283593;
  --color-prussian: #0d47a1;
  --color-horizon: #42a5f5;
  --color-violet: #5e35b1;
  --color-lavender: #7e57c2;

  /* Wheat Field */
  --color-harvest: #ff8f00;
  --color-wheat: #ffb300;
  --color-grain: #ffd54f;
  --color-straw: #ffecb3;
  --color-ochre: #e65100;
  --color-sienna: #8d3a14;

  /* Earth & Path */
  --color-field: #2e7d32;
  --color-sage: #66bb6a;
  --color-olive: #827717;
  --color-umber: #4e342e;
  --color-clay: #6d4c41;
  --color-earth: #3e2723;

  /* Crow & Shadow */
  --color-crow: #121212;
  --color-shadow: #1e1e2f;
  --color-slate: #37474f;
  --color-pewter: #546e7a;
  --color-cloud: #90a4ae;
  --color-misty: #cfd8dc;

  /* Status */
  --color-error: #c62828;
  --color-coral: #ff7043;
  --color-success: #2e7d32;
  --color-teal: #00897b;
  --color-info: #00acc1;
  --color-special: #ad1457;
  --color-lime: #9e9d24;
}
```

## TypeScript Constants

```typescript
export const colors = {
  // Primary
  rust: '#bf360c',
  cobalt: '#1565c0',
  wheat: '#ffb300',
  night: '#0a0a0f',
  charcoal: '#1a1a2e',

  // Stormy Sky
  storm: '#1a237e',
  twilight: '#283593',
  prussian: '#0d47a1',
  horizon: '#42a5f5',
  violet: '#5e35b1',
  lavender: '#7e57c2',

  // Wheat Field
  harvest: '#ff8f00',
  wheat: '#ffb300',
  grain: '#ffd54f',
  straw: '#ffecb3',
  ochre: '#e65100',
  sienna: '#8d3a14',

  // Earth & Path
  field: '#2e7d32',
  sage: '#66bb6a',
  olive: '#827717',
  umber: '#4e342e',
  clay: '#6d4c41',
  earth: '#3e2723',

  // Crow & Shadow
  crow: '#121212',
  shadow: '#1e1e2f',
  slate: '#37474f',
  pewter: '#546e7a',
  cloud: '#90a4ae',
  misty: '#cfd8dc',

  // Status
  error: '#c62828',
  coral: '#ff7043',
  success: '#2e7d32',
  teal: '#00897b',
  info: '#00acc1',
  special: '#ad1457',
  lime: '#9e9d24',
} as const;

export const chartSequence = [
  '#bf360c', '#1565c0', '#ffb300', '#2e7d32', '#5e35b1', '#ff8f00',
  '#00897b', '#c62828', '#7e57c2', '#827717', '#00acc1', '#ad1457',
] as const;
```
