# Pretty Diff for `micromegas-screens plan` Updates

## Overview

When `micromegas-screens plan` detects screens that would be updated, it currently only prints `~ update: screen-name` with no indication of *what* changed. For screens with large configs (notebooks with many cells), this makes it difficult to review changes before running `apply`. The plan command should display a readable diff of the JSON content for each updated screen, similar to `terraform plan` or `git diff`.

## Current State

The `plan` flow works as follows:

1. `cmd_plan()` calls `compute_plan()` which returns five lists of **screen names**: `creates`, `updates`, `deletes`, `unchanged`, `untracked` (`screens.py:276-318`)
2. `format_plan()` renders these name lists into text (`screens.py:321-344`)
3. For updates, it only prints `~ update: {name}` — no content comparison

The comparison logic lives in `screens_equal()` (`screens.py:88-90`) which strips volatile keys and checks equality, but discards the actual diffs. Both the local data and server data are available inside `compute_plan()` but are not propagated out.

`cmd_apply()` also calls `format_plan()` to show what it's about to do before prompting for confirmation — so it would also benefit from diffs.

## Design

### Diff approach: unified diff of pretty-printed JSON

Use Python's `difflib.unified_diff` on the pretty-printed JSON of each updated screen (after stripping volatile keys). This:
- Requires no new dependencies (stdlib only)
- Produces output developers already know how to read
- Naturally handles nested config changes (cells, queries, etc.)

### Data flow change

`compute_plan()` currently returns names only. For updates, it needs to also return the local and server screen dicts so the diff can be computed at display time.

```
# Current signature:
compute_plan(...) -> (creates, updates, deletes, unchanged, untracked)
#                     list     list     list     list        list
#                     of names

# New signature:
compute_plan(...) -> (creates, updates, deletes, unchanged, untracked)
#                     list     list of  list     list        list
#                     of names tuples   of names of names    of names
```

Each entry in `updates` changes from a plain name string to a tuple: `(name, local_dict, server_dict)` — both dicts already stripped of volatile keys.

### Diff rendering

A new helper `format_screen_diff(name, server_dict, local_dict)` produces a unified diff:

```
  ~ update: my-notebook
    --- server
    +++ local
    @@ ... @@
       "config": {
         "cells": [
    -      {"type": "markdown", "content": "old text"}
    +      {"type": "markdown", "content": "new text"}
         ]
       }
```

- The diff lines are indented by 4 spaces to sit under the `~ update:` line
- When stdout is a TTY, colorize: red for removals, green for additions, cyan for `@@` headers
- When piped or redirected, output plain text (no ANSI codes)

### ANSI color

A small helper checks `sys.stdout.isatty()` and wraps lines with ANSI codes:
- `\033[31m` (red) for `-` lines
- `\033[32m` (green) for `+` lines
- `\033[36m` (cyan) for `@@` lines
- `\033[0m` reset

No color library needed — these are standard terminal codes.

## Implementation Steps

All changes are in two files:

### 1. Update `compute_plan()` to return screen data for updates

In `screens.py:276-318`, change the `updates` list to collect `(name, stripped_local, stripped_server)` tuples instead of just names.

### 2. Add `format_screen_diff()` helper

New function that takes a name, server dict, and local dict, produces pretty-printed JSON for each, runs `difflib.unified_diff`, and returns formatted/colorized lines.

### 3. Update `format_plan()` to render diffs

Modify `format_plan()` to:
- Accept the new update tuples (from step 1)
- Accept a `use_color` parameter (default: `sys.stdout.isatty()`)
- Call `format_screen_diff()` for each update entry, passing `use_color`
- The summary line at the bottom still counts `len(updates)`

### 4. Update callers of `compute_plan()` and `format_plan()`

Both `cmd_plan()` and `cmd_apply()` call `format_plan()` and must pass the `use_color` flag:

- `cmd_plan()` (`screens.py:347-356`): pass `use_color=not args.no_color` to `format_plan()`
- `cmd_apply()` (`screens.py:359-438`): pass `use_color=not args.no_color` to `format_plan()`, and update the loop over `updates` to unpack tuples. The apply loop needs just the name, so unpack with `for name, _, _ in updates:` (or extract names separately)

### 5. Add `--no-color` flag (optional)

Add a `--no-color` flag to `plan` and `apply` subcommands that forces plain output regardless of TTY detection. Pass through to `format_plan()`.

### 6. Update tests

In `test_screen_files.py`:
- Update `TestComputePlan` assertions: `updates` entries are now tuples `(name, local, server)` instead of bare names
- Add test for `format_screen_diff()` verifying it produces expected unified diff output
- Add test for colorized vs plain output

## Files to Modify

| File | Change |
|------|--------|
| `python/micromegas/micromegas/cli/screens.py` | Update `compute_plan()`, add `format_screen_diff()`, update `format_plan()`, update `cmd_apply()` loop |
| `python/micromegas/tests/test_screen_files.py` | Update existing assertions, add diff formatting tests |

## Trade-offs

**Unified text diff vs structural diff (deepdiff)**: A structural diff library would show semantic paths like `config.cells[0].content: "old" → "new"`, which is more precise. But it adds a dependency, produces unfamiliar output format, and doesn't handle reordered arrays well. Unified diff of pretty JSON is familiar to all developers and uses only stdlib.

**Returning tuples vs a dataclass**: Tuples are slightly less readable but avoid adding a new class for a three-element structure. If the plan output grows more complex in the future, a `PlanEntry` dataclass could replace the tuple — but for now YAGNI.

**Color**: ANSI codes directly vs a library like `colorama`. Direct ANSI is simpler, works on all Unix terminals and modern Windows Terminal/WSL. No need for a dependency.

## Testing Strategy

- Existing `TestComputePlan` tests continue to pass (with updated tuple unpacking)
- New unit test: `format_screen_diff()` with known local/server dicts produces expected diff lines
- New unit test: color is applied when `use_color=True`, absent when `False`
- Manual test: run `micromegas-screens plan` against a server with modified screens, verify diff output is readable and correctly colored

## Open Questions

None — this is a self-contained change with a clear approach.
