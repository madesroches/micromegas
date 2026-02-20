# Compact Notebook UI — Borderless Design

**Status:** DRAFT

## Overview

Make the notebook UI more data-dense by switching to a borderless design with thin section dividers. Groups become `── INGESTION ──` horizontal rules instead of boxed panels with heavy headers. Child cell names and status become small inline labels above content. All editing controls (drag handles, run buttons, menus) appear on hover only.

See `mockup-borderless.html` (proposed) and `mockup-current.html` (reference) in this folder.

## Current State

The notebook layout has multiple layers of chrome stacking vertically:

```
┌──────────────────────────────────────────────┐
│ ⠁⠂ ▼  HG  Ingestion              ▶ ⋮  │  ← Group header: px-3 py-2, bg-app-card, border-2
├────────────────────┬─────────────────────────┤
│ ⠁⠂ MET metrics  ▶⋮│ ⠁⠂ LOG warnings   ▶⋮│  ← Child headers: px-2 py-1.5 each
│  [chart content]   │  [log content]          │
└────────────────────┴─────────────────────────┘
         gap-3 = 12px
```

**Overhead per group:**
- Outer container: `border-2` (4px total), `rounded-lg`
- Group header (CellContainer): `px-3 py-2` + `border-b` + `bg-app-card` ≈ 25px
- HG content padding: `p-4` (16px on each side)
- Child cell border: `border rounded-md` (2px total)
- Child header: `px-2 py-1.5` + `border-b` ≈ 22px
- Child content padding: `p-2` (8px each side)
- Gap between groups: `gap-3` = 12px

**Total chrome per group: ~75px of vertical space not showing data.**

### Key files

| File | Role |
|------|------|
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | Top-level layout, `p-6`, `gap-3`, renders cells |
| `analytics-web-app/src/components/CellContainer.tsx` | Cell wrapper: `border-2`, header with `px-3 py-2` |
| `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | HG children: `gap-2`, child headers `px-2 py-1.5`, content `p-2` |

## Design

### Target layout

```
── INGESTION ──────────────────────────────────
  ingestion_metrics · 1,024 rows   │  ingestion_warnings · 48 rows
  [chart content]                  │  [log content]

── FLIGHTSQL ── flightsql_metrics, flightsql_warnings ──  (collapsed)

  ▶ process_list · 15 rows (1 KB) in 0.2s               (collapsed cell)
```

### Changes by component

#### 1. NotebookRenderer — main container

| Property | Current | New |
|----------|---------|-----|
| Outer padding | `p-6` (24px) | `p-2` (8px) |
| Cell gap | `gap-3` (12px) | `gap-1` (4px) |

#### 2. CellContainer — group/cell wrapper

This is the biggest change. The thick bordered box with a full header bar becomes:

**Expanded group (HG):**
- Replace `border-2 rounded-lg bg-app-panel` box with no border, no background
- Replace full header bar with a thin section divider:
  - Layout: `flex items-center gap-2 py-1 px-1`
  - Collapse chevron (always visible)
  - Group name: `text-[11px] font-semibold text-theme-text-muted uppercase tracking-wide`
  - Horizontal rule: `flex-1 h-px bg-theme-border`
  - Run + menu buttons: visible on hover only (`opacity-0 group-hover:opacity-100`)

**Collapsed group:**
- Same divider, but children names listed inline after the group name
- `── FLIGHTSQL ── flightsql_metrics, flightsql_warnings ──`

**Expanded regular cell:**
- Small pane label above content (like in child cells below)
- No surrounding box

**Collapsed regular cell:**
- Single line: `▶ cell_name · status_text`, controls on hover
- Subtle background: `bg-app-panel rounded`
- Height: ~20px

**Variable cells (auto-collapsed with titleBarContent):**
- Minimal inline row: `name [select dropdown]` with no border
- Same height as current but no box chrome

#### 3. HorizontalGroupCell — children layout

| Property | Current | New |
|----------|---------|-----|
| Content padding | `p-4` (inside CellContainer) | `0` (no CellContainer padding) |
| Children gap | `gap-2` (8px) | `gap-px` (1px, just a thin divider line) |
| Child border | `border rounded-md` | None |
| Child header | `px-2 py-1.5 bg-app-card border-b` (22px) | Pane label: `px-2 py-0.5 text-[10px]` (~16px) |
| Child content padding | `p-2` (8px) | `px-1 pb-1` (4px sides, 4px bottom) |

**Child pane label format:**
```
cell_name · status_text                          [▶] [⋮]  (hover)
```
- Name: `text-[10px] font-medium text-theme-text-secondary`
- Separator: `·` in `text-theme-border`
- Status: `text-[10px] text-theme-text-muted`
- Controls: `opacity-0` → `opacity-100` on pane hover

**Vertical divider between children:**
- Replace `gap-2` with `gap-px` and `border-r border-theme-border` on each child except last

### Selection state

The current selection uses `border-[var(--selection-border)]` on the CellContainer box. In the borderless design:

- **Group selected:** Thin left accent bar on the section divider (`border-l-2 border-accent-link pl-1`)
- **Child cell selected:** Subtle highlight on the pane label row (`bg-[var(--selection-bg)]`) + left accent border on the content pane

### Drag & drop

Drag handles currently live in the headers. In the borderless design:
- **Group drag:** Grip icon appears on hover to the left of the section divider chevron
- **Child drag (within HG):** Grip icon appears on hover at the start of the pane label
- **DragOverlay:** Keep existing preview styling (it's a floating element, not affected by layout)

### Error and blocked states

- **Error state:** Keep `bg-[var(--error-bg)] border border-accent-error rounded-md` inside content area (unchanged)
- **Blocked state:** Keep current dashed border message (unchanged)
- **Error status text:** Shown in red in the pane label, same as current

## Implementation Steps

### Phase 1: Spacing reduction (low risk, immediate improvement)

1. **NotebookRenderer.tsx** — Change main container:
   - `p-6` → `p-2`
   - `gap-3` → `gap-1`

2. **CellContainer.tsx** — Reduce content padding:
   - Content area `p-4` → `p-1` for regular cells
   - Content area `p-4` → `p-0` for HG cells (type === 'hg')

3. **HorizontalGroupCell.tsx** — Tighten children:
   - Container `gap-2` → `gap-px`
   - Child content `p-2` → `px-1 pb-1`

### Phase 2: Borderless containers

4. **CellContainer.tsx** — Replace boxed rendering with borderless:
   - Root: remove `border-2 rounded-lg bg-app-panel`
   - Header: replace with section divider layout
   - Selection: left accent bar instead of border color change

5. **CellContainer.tsx** — Collapsed state:
   - Group collapsed: divider line with inline children names (needs `childNames?: string[]` prop)
   - Cell collapsed: single compact line with chevron + name + status

### Phase 3: Compact child headers

6. **CellContainer.test.tsx** — Update tests for borderless rendering:
   - Fix DOM selector `div[class*="bg-app-panel"]` (line 125) — root no longer has `bg-app-panel`
   - Fix selection assertions `border-[var(--selection-border)]` (line 287) — now uses left accent bar
   - Update any assertions that depend on removed classes or changed DOM structure

7. **HorizontalGroupCell.tsx** — Replace `ChildCellHeader` with compact pane label:
   - Remove drag handle from always-visible (hover only)
   - Remove type badge
   - Show `name · status` in single `text-[10px]` row
   - Controls on hover
   - Remove child border and background

8. **HorizontalGroupCell.tsx** — Vertical divider between children:
   - Add `border-r border-theme-border` on children except `:last-child`

### Phase 4: Variable cells

9. **CellContainer.tsx** — Variable cell styling:
   - When `titleBarContent` is present:
     - Render as minimal inline row: `name [widget]` with no box chrome
     - Reduce vertical padding to minimum

### Phase 5: Selection & interaction polish

10. **CellContainer.tsx** — Selection indicator:
    - Left accent bar for selected state
    - Ensure click target areas are still large enough

11. **HorizontalGroupCell.tsx** — Selection on child panes:
    - Subtle background highlight on selected child

## Files to Modify

| File | Changes |
|------|---------|
| `analytics-web-app/src/components/CellContainer.tsx` | Borderless rendering, section divider header, compact collapsed states |
| `analytics-web-app/src/lib/screen-renderers/cells/HorizontalGroupCell.tsx` | Compact pane labels, remove child borders, vertical dividers, tighter spacing |
| `analytics-web-app/src/lib/screen-renderers/NotebookRenderer.tsx` | Reduce `p-6`→`p-2`, `gap-3`→`gap-1` |
| `analytics-web-app/src/components/__tests__/CellContainer.test.tsx` | Update DOM selectors and selection assertions for borderless rendering |

## Testing Strategy

- Visual comparison: open `mockup-borderless.html` side-by-side with running app
- Verify all cell types render correctly: SQL, metrics, log, table, variable, HG
- Test collapsed/expanded toggle for groups and cells
- Test drag & drop: reorder cells, drag children out of groups
- Test selection: click cells, verify editor panel opens
- Test status display: run cells, verify status text updates
- Test error state: trigger query error, verify red status and error content
- Run existing tests: `cd analytics-web-app && yarn test`

## Decisions

- No fallback/variant toggle — one borderless design for all cases
- Add Cell button at the bottom is fine as-is
- Editor panel is out of scope for now
