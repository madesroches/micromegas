#!/usr/bin/env python3
"""Drift guard: every binary that declares jemalloc as its global allocator
must also invoke `declare_jemalloc_conf!()`, so a ninth binary added later
can't silently ship without the tuned decay/background-thread config.
See `mkdocs/docs/admin/memory-allocator.md`.
"""

import pathlib
import re
import sys

GLOBAL_ALLOCATOR_RE = re.compile(
    r"#\[global_allocator\]\s*\nstatic\s+\w+\s*:\s*tikv_jemallocator::Jemalloc"
)
DECLARE_MACRO_RE = re.compile(r"declare_jemalloc_conf!")


def main():
    root = pathlib.Path(__file__).parent.parent.absolute()
    offenders = []
    for path in sorted((root / "rust").glob("*/src/**/*.rs")):
        text = path.read_text()
        if GLOBAL_ALLOCATOR_RE.search(text) and not DECLARE_MACRO_RE.search(text):
            offenders.append(path)

    if offenders:
        print("Binaries declaring jemalloc as global allocator without declare_jemalloc_conf!():")
        for path in offenders:
            print(f"  {path.relative_to(root)}")
        print("\nAdd `micromegas::declare_jemalloc_conf!();` immediately below the")
        print("`#[global_allocator]` static -- see mkdocs/docs/admin/memory-allocator.md.")
        sys.exit(1)
    else:
        print("All jemalloc global-allocator binaries declare a malloc_conf.")


if __name__ == "__main__":
    main()
