# Swimlane Cell Type Implementation Plan

## Overview

Add a new `swimlane` notebook cell type that displays horizontal swimlane visualizations with time segment bars. This generalizes the existing `ThreadCoverageTimeline` component as a reusable, SQL-driven notebook cell.

**GitHub Issue**: #763

## Data Schema

The cell expects query results with these columns:

| Column | Type | Description |
|--------|------|-------------|
| `id` | string | Unique identifier for the lane |
| `name` | string | Display name for the lane |
| `begin` | timestamp | Segment start time |
| `end` | timestamp | Segment end time |

Multiple rows with the same `id`/`name` create multiple segments in a single lane. Lane order follows first occurrence in query results (use `ORDER BY` to control).

## Files to Change

**Create:**
- `analytics-web-app/src/lib/screen-renderers/cells/SwimlaneCell.tsx`

**Modify:**
- `notebook-types.ts` - Add `'swimlane'` to `CellType` and `QueryCellConfig.type` unions
- `cell-registry.ts` - Import and register `swimlaneMetadata`
- `notebook-utils.ts` - Add default SQL template

## Implementation Approach

Follow existing patterns:
- **Cell structure**: `PropertyTimelineCell.tsx` (renderer, editor, metadata in single file)
- **Rendering**: `ThreadCoverageTimeline.tsx` (lane layout, segment positioning, drag-to-zoom)
- **Timestamps**: Use `timestampToMs` from `arrow-utils.ts`

## Metadata

- `label`: "Swimlane"
- `icon`: "S"
- `description`: "Horizontal lanes with time segments"
- `defaultHeight`: 300
- `canBlockDownstream`: true

## Testing

1. Create a swimlane cell with thread coverage query
2. Verify lanes display correctly
3. Test drag-to-zoom updates time range
4. Test with empty query results
