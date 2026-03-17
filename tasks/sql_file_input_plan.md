# micromegas-query: Accept SQL File Path Plan

## Overview

Add a `--file` option to the `micromegas-query` CLI so users can provide SQL from a `.sql` file instead of an inline string argument. This improves ergonomics for complex queries involving multi-line SQL, JSONPath expressions, and special characters that require careful shell escaping.

GitHub issue: #941

## Current State

The CLI is implemented in `python/micromegas/micromegas/cli/query.py`. It uses `argparse` with `sql` as a required positional argument:

```python
parser.add_argument("sql", help="SQL query to execute")
```

The SQL string is passed directly to the client:

```python
client = connection.connect()
df = client.query(args.sql, begin, end)
```

Usage today:
```bash
micromegas-query "SELECT * FROM log_entries LIMIT 10" --begin 1h
```

## Design

Make the `sql` positional argument optional and add a `--file` flag. Exactly one of the two must be provided.

argparse supports this via `nargs="?"` on the positional argument. Validation ensures mutual exclusivity:
- If `--file` is given, read SQL from that path
- If positional `sql` is given, use it as today
- If both or neither are provided, error out

Also support reading from stdin via `--file -`, which is a common CLI convention and enables piping.

## Implementation Steps

1. **Modify argument parser** in `python/micromegas/micromegas/cli/query.py`:
   - Add `import sys` and `import pathlib` (needed for stdin and file reading)
   - Change `sql` positional argument to `nargs="?"` (optional) with `default=None`
   - Add `--file` argument with help text explaining it accepts a file path or `-` for stdin

2. **Add SQL resolution logic** after `args = parser.parse_args()`:
   - If both `args.file` and `args.sql` are provided, call `parser.error()`
   - If neither is provided, call `parser.error()`
   - If `--file` is given:
     - If value is `-`, read from `sys.stdin.read()`
     - Otherwise, read the file contents with `pathlib.Path(args.file).read_text()`, wrapped in a try/except to catch `OSError` and call `parser.error()` with a user-friendly message (e.g., `f"cannot read file '{args.file}': {e}"`)
   - Strip trailing whitespace/newlines from file-sourced SQL

3. **Update help text** for the `sql` argument to mention the `--file` alternative.

4. **Update documentation** in `mkdocs/docs/query-guide/python-api.md`:
   - In the "query.py - Run SQL Queries" section (~line 563), update the `sql` argument description to note it is optional when `--file` is used
   - Add `--file` to the **Options** list with description: accepts a file path or `-` for stdin
   - Add examples for file input and stdin piping to the **Examples** section

5. **Update query skill** in `claude-plugin/skills/micromegas-query/SKILL.md`:
   - Update the CLI syntax block (~line 57) to include `--file` option: `micromegas-query [--file <path|->] ["<SQL>"] --begin <time> ...`
   - Add a bullet under the syntax section explaining `--file` accepts a `.sql` file path or `-` for stdin, as an alternative to the inline SQL positional argument
   - Note that using `--file` avoids awkward shell quoting when queries contain JSONPath expressions (e.g., `$[*].attributes[*]?(@.key=="Branch").value`)

## Files to Modify

- `python/micromegas/micromegas/cli/query.py` — argument parsing and SQL resolution
- `mkdocs/docs/query-guide/python-api.md` — CLI documentation (query.py section)
- `claude-plugin/skills/micromegas-query/SKILL.md` — query skill CLI syntax section

## Trade-offs

**Chosen: `--file` flag alongside optional positional argument**
- Preserves full backward compatibility — existing inline usage is unchanged
- Clear and explicit — `--file query.sql` reads like natural CLI usage

**Alternative considered: auto-detect if positional arg is a file path**
- Rejected: ambiguous (a SQL string could coincidentally match a filename), violates principle of least surprise

**Alternative considered: mutually exclusive argparse group**
- argparse's `add_mutually_exclusive_group` doesn't cleanly handle mixing positional and optional arguments, so manual validation is simpler and produces better error messages

## Testing Strategy

Note: there are no existing automated CLI tests in the project, so manual testing is sufficient for this change.

- Manual testing:
  - `micromegas-query "SELECT 1" --all` (existing usage, unchanged)
  - `micromegas-query --file query.sql --begin 1h` (new file input)
  - `echo "SELECT 1" | micromegas-query --file - --all` (stdin)
  - `micromegas-query --all` (error: no SQL provided)
  - `micromegas-query "SELECT 1" --file query.sql --all` (error: both provided)
  - `micromegas-query --file nonexistent.sql --all` (error: file not found)

## Open Questions

None — the scope is straightforward.
