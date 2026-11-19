"""
Copy the canonical MCP skills into the Claude plugin.

The skills live inside the `micromegas` crate (rust/public/src/servers/mcp/skills) because
`include_str!` outside the crate directory breaks `cargo publish`. `claude-plugin/skills/` holds
plain copies, not symlinks. Run this after editing a canonical skill; a Rust test fails when the
two differ.
"""

import pathlib
import shutil

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
CANONICAL = REPO_ROOT / "rust" / "public" / "src" / "servers" / "mcp" / "skills"
PLUGIN = REPO_ROOT / "claude-plugin" / "skills"


def main() -> None:
    for skill_dir in sorted(p for p in CANONICAL.iterdir() if p.is_dir()):
        target = PLUGIN / skill_dir.name
        target.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(skill_dir / "SKILL.md", target / "SKILL.md")
        print(f"copied {skill_dir.name}")


if __name__ == "__main__":
    main()
