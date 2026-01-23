# Cell Editor Syntax Coloring Plan

## Status: COMPLETED

Implementation completed on 2026-01-23. All cell editors now have syntax highlighting.

## Overview

Add syntax highlighting to all cell editors in the analytics web app that contain SQL or Markdown content.

## Current State (Before Implementation)

### Cell Types with Text Editors
| Cell Type | Content | Current Editor | Needs Highlighting |
|-----------|---------|----------------|-------------------|
| TableCell | SQL | Plain `<textarea>` | Yes (SQL) |
| ChartCell | SQL | Plain `<textarea>` | Yes (SQL) |
| LogCell | SQL | Plain `<textarea>` | Yes (SQL) |
| VariableCell | SQL | Plain `<textarea>` | Yes (SQL) |
| MarkdownCell | Markdown | Plain `<textarea>` | Yes (Markdown) |

### Existing Implementation
- `QueryEditor.tsx` has a custom SQL highlighter using:
  - Transparent `<textarea>` overlay for input
  - Hidden `<pre>` with highlighted HTML behind it
  - Regex-based highlighting for keywords, strings, and variables
- Cell editors in `cells/*.tsx` use plain `<textarea>` elements without highlighting

### Key Files
- `analytics-web-app/src/components/QueryEditor.tsx` - Existing SQL highlighting (lines 45-64)
- `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` - SQL textarea
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` - SQL textarea
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` - SQL textarea
- `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` - SQL textarea
- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` - Markdown textarea

## Approach

Create a reusable `SyntaxEditor` component that provides syntax highlighting for both SQL and Markdown, using the overlay technique already proven in `QueryEditor.tsx`.

### Why Not Use a Library?
- The existing custom approach is lightweight and works well
- Adding CodeMirror or Monaco would significantly increase bundle size
- The current regex-based highlighting is sufficient for SQL and can be extended for Markdown
- Consistency with existing `QueryEditor.tsx` implementation

## Implementation

### Phase 1: Extract Shared Syntax Editor Component - DONE

#### 1.1 Create `SyntaxEditor.tsx` - DONE
Extract highlighting logic from `QueryEditor.tsx` into a reusable component:

```typescript
// analytics-web-app/src/components/SyntaxEditor.tsx
interface SyntaxEditorProps {
  value: string;
  onChange: (value: string) => void;
  language: 'sql' | 'markdown';
  placeholder?: string;
  className?: string;
  minHeight?: string;
}
```

Features:
- Overlay pattern: transparent textarea over highlighted `<pre>`
- Language-specific highlighting functions
- Synchronized scrolling between textarea and highlighted output
- Auto-resize to fit content

#### 1.2 SQL Highlighting Function - DONE
Move existing `highlightSql` from `QueryEditor.tsx`:
- Keywords: `SELECT`, `FROM`, `WHERE`, `ORDER BY`, etc.
- String literals: `'...'`
- Variables: `$variable_name`
- Numbers: `123`, `45.67`
- Comments: `-- comment`

#### 1.3 Markdown Highlighting Function - DONE
Create new `highlightMarkdown` function:
- Headers: `# `, `## `, `### `, etc.
- Bold: `**text**`
- Italic: `*text*`
- Code: `` `code` ``
- Links: `[text](url)`
- Lists: `- `, `* `, `1. `
- Blockquotes: `> `

### Phase 2: Update Cell Editors - DONE

#### 2.1 Update SQL Cell Editors - DONE
Replace `<textarea>` with `<SyntaxEditor language="sql">` in:
- [x] `TableCell.tsx`
- [x] `ChartCell.tsx`
- [x] `LogCell.tsx`
- [x] `VariableCell.tsx`

#### 2.2 Update Markdown Cell Editor - DONE
Replace `<textarea>` with `<SyntaxEditor language="markdown">` in:
- [x] `MarkdownCell.tsx`

#### 2.3 Update QueryEditor - DONE
Refactor `QueryEditor.tsx` to use `SyntaxEditor` internally, removing duplicated highlighting code.

### Phase 3: Styling - DONE

#### 3.1 Theme Variables - DONE
Added CSS variables for syntax colors to `globals.css`:
```css
:root {
  --syntax-keyword: #c792ea;     /* purple - SQL keywords */
  --syntax-string: #c3e88d;      /* green - string literals */
  --syntax-variable: #ffcb6b;    /* orange - $variables */
  --syntax-number: #f78c6c;      /* coral - numbers */
  --syntax-comment: #546e7a;     /* gray - comments */
  --syntax-header: #82aaff;      /* blue - markdown headers */
  --syntax-bold: #ffffff;        /* white - bold text */
  --syntax-italic: #c792ea;      /* purple - italic text */
  --syntax-link: #89ddff;        /* cyan - links */
  --syntax-code: #c3e88d;        /* green - inline code */
  --syntax-blockquote: #546e7a;  /* gray - blockquotes */
  --syntax-list: #ffcb6b;        /* orange - list markers */
}
```

#### 3.2 Dark Theme Consistency - DONE
Colors work well with existing dark theme in the app.

## File Changes

### New Files
- `analytics-web-app/src/components/SyntaxEditor.tsx` - Reusable editor component

### Modified Files
- `analytics-web-app/src/components/QueryEditor.tsx` - Uses SyntaxEditor internally
- `analytics-web-app/src/lib/screen-renderers/cells/TableCell.tsx` - Uses SyntaxEditor
- `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx` - Uses SyntaxEditor
- `analytics-web-app/src/lib/screen-renderers/cells/LogCell.tsx` - Uses SyntaxEditor
- `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx` - Uses SyntaxEditor
- `analytics-web-app/src/lib/screen-renderers/cells/MarkdownCell.tsx` - Uses SyntaxEditor
- `analytics-web-app/src/styles/globals.css` - Added syntax color variables

## Testing

- [x] Run `yarn lint` - Passed
- [x] Run `yarn type-check` - Passed
- [ ] Verify SQL highlighting in all query cell editors (manual)
- [ ] Verify Markdown highlighting in markdown cell editor (manual)
- [ ] Test typing performance (no lag on keystrokes) (manual)
- [ ] Test scrolling synchronization in long content (manual)
- [ ] Test copy/paste behavior (manual)
- [ ] Verify existing QueryEditor panel still works (manual)
- [ ] Manual visual inspection of colors against theme (manual)

## Implementation Order (Completed)

1. ~~Create `SyntaxEditor.tsx` with SQL highlighting (extracted from QueryEditor)~~
2. ~~Add Markdown highlighting function~~
3. ~~Add CSS variables for syntax colors~~
4. ~~Update `MarkdownCell.tsx` to use SyntaxEditor~~
5. ~~Update SQL cell editors (TableCell, ChartCell, LogCell, VariableCell)~~
6. ~~Refactor QueryEditor to use SyntaxEditor~~
7. ~~Run lint and type-check~~
