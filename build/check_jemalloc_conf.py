#!/usr/bin/env python3
"""Drift guard: every binary that declares jemalloc as its global allocator
must also invoke `declare_jemalloc_conf!()`, so a ninth binary added later
can't silently ship without the tuned decay/background-thread config.
See `mkdocs/docs/admin/memory-allocator.md`.
"""

import pathlib
import re
import sys

# The static declaration a `#[global_allocator]` attribute applies to, allowing
# intervening attributes (e.g. `#[cfg(...)]`) between the two. Only requires a
# `Jemalloc` mention somewhere in the declaration itself (type or initializer),
# so both `static X: tikv_jemallocator::Jemalloc = ...Jemalloc;` and an aliased
# `use tikv_jemallocator::Jemalloc; ... static X: Jemalloc = Jemalloc;` match.
GLOBAL_ALLOCATOR_STATIC_RE = re.compile(
    r"#\[global_allocator\](?:\s*#\[[^\]]*\])*\s*(?:pub\s+)?static\s+\w+\s*:[^;]*;",
    re.MULTILINE,
)
JEMALLOC_TYPE_RE = re.compile(r"\bJemalloc\b")
DECLARE_MACRO_RE = re.compile(r"declare_jemalloc_conf!")


def main():
    root = pathlib.Path(__file__).parent.parent.absolute()
    found = 0
    offenders = []
    for path in sorted((root / "rust").glob("**/src/**/*.rs")):
        if "target" in path.relative_to(root).parts:
            continue
        text = path.read_text()
        declares_jemalloc = any(
            JEMALLOC_TYPE_RE.search(m.group())
            for m in GLOBAL_ALLOCATOR_STATIC_RE.finditer(text)
        )
        if not declares_jemalloc:
            continue
        found += 1
        if not DECLARE_MACRO_RE.search(text):
            offenders.append(path)

    if found == 0:
        print("No jemalloc global-allocator declarations found -- the scan is broken.")
        sys.exit(1)

    if offenders:
        print("Binaries declaring jemalloc as global allocator without declare_jemalloc_conf!():")
        for path in offenders:
            print(f"  {path.relative_to(root)}")
        print("\nAdd `micromegas::declare_jemalloc_conf!();` immediately below the")
        print("`#[global_allocator]` static -- see mkdocs/docs/admin/memory-allocator.md.")
        sys.exit(1)
    else:
        print(f"All {found} jemalloc global-allocator binaries declare a malloc_conf.")


if __name__ == "__main__":
    main()
