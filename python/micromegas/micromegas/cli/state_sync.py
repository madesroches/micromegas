"""Shared diff/prompt/color helpers for the terraform-shaped `plan`/`apply` CLIs
(`micromegas-screens`, `micromegas-views`).

Presentation and prompts only: the plan model itself (what `compute_plan` returns, ownership
markers, ...) is not generalized here and stays private to each tool.
"""

import argparse
import difflib
import sys


def unified_diff(before_lines, after_lines, from_label, to_label, use_color):
    """Return a 4-space-indented unified diff between two line lists, or "" when equal.

    Colorizes `---`/`+++` bold, `@@` cyan, `-` red, `+` green when `use_color` is set.
    """
    diff_lines = list(
        difflib.unified_diff(
            before_lines, after_lines, fromfile=from_label, tofile=to_label
        )
    )
    if not diff_lines:
        return ""
    result = []
    for line in diff_lines:
        if use_color:
            if line.startswith("---") or line.startswith("+++"):
                line = f"\033[1m{line}\033[0m"
            elif line.startswith("@@"):
                line = f"\033[36m{line}\033[0m"
            elif line.startswith("-"):
                line = f"\033[31m{line}\033[0m"
            elif line.startswith("+"):
                line = f"\033[32m{line}\033[0m"
        result.append(f"    {line}")
    return "\n".join(result)


def confirm_apply(auto_approve):
    """Prompt `[y/N]` unless `auto_approve`. Returns True when approved (or auto-approved),
    False on a declined prompt -- printing "Apply cancelled." in that case. Never exits the
    process; the caller decides what a decline means.
    """
    if auto_approve:
        return True
    answer = input("Do you want to apply these changes? [y/N]: ").strip().lower()
    if answer != "y":
        print("Apply cancelled.")
        return False
    return True


def add_color_arg(parser):
    """Add the shared `--color`/`--no-color` flag, default on."""
    parser.add_argument(
        "--color",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Colored diff output",
    )


def use_color(args):
    """Whether to colorize diff output: only when stdout is a tty and `--color` wasn't
    turned off. Named `use_color` rather than `colorize` so both tools' call sites (which
    already have a local of that name) must rename their own local to avoid shadowing this
    function -- see call sites for the rationale.
    """
    return sys.stdout.isatty() and args.color
