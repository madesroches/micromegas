# Release Plan: Python client v0.31.1 (PyPI only)

## Overview

Publish the `micromegas` Python package to PyPI as **0.31.1**, an exceptional out-of-band patch
release carrying the two `micromegas-setup-telemetry` fixes that landed after v0.31.0. Nothing else
ships: no crates.io publish, no Docker images, no Grafana plugin, no GitHub release, and no change
to the repo's in-tree 0.32.0 development version. The work happens on a throw-away branch whose
only lasting outputs are the PyPI artifact, a `python-v0.31.1` tag, and a small CHANGELOG record
landed on `main` through its own PR.

## Current State

### What would ship

`git diff micromegas-v0.31.0..HEAD -- python/` is exactly three files:

| File | Source |
| --- | --- |
| `python/micromegas/micromegas/cli/setup_telemetry.py` (+199) | #1591, #1593 |
| `python/micromegas/tests/cli/test_setup_telemetry.py` (+394) | #1591, #1593 |
| `python/micromegas/pyproject.toml` (version 0.31.0 → 0.32.0) | #1583 post-release bump |

So `main`'s python tree *is* the 0.31.0 python tree plus those two commits, and nothing else. There
is no third python change hiding in the range, which is why a branch off `main` HEAD is equivalent
to a cherry-pick onto the `micromegas-v0.31.0` tag with none of the conflict risk.

The two changes are:
- **#1591** — `--env-file` crashed with `AttributeError: module 'os' has no attribute 'fchmod'` on
  Windows before Python 3.13, leaving a 0-byte env file and a just-minted, never-retrievable
  ingestion API key lost. This is the reason to cut the release out of band.
- **#1593** — `--format {posix,powershell,cmd,dotenv}`, plus `posix` switching to `shlex.quote`
  (closing a latent mis-`eval` on an endpoint carrying a shell metacharacter).

### Server-compatibility drift: none

`git diff micromegas-v0.31.0..HEAD -- rust/` is version bumps, `rust-toolchain.toml` 1.98.1, and one
clippy fix in `transit/src/dyn_string.rs`. No FlightSQL, HTTP, or wire-format change, so a client
built from `main` HEAD talks to a 0.31.0 server exactly as the 0.31.0 client does.

### Existing release tooling — what is and isn't reusable

- **`build/release.py`** — 16 `cargo release` invocations in dependency order. Crates only, no
  python step. **Not used.**
- **`.github/workflows/python.yml`** — CI only: a 3.11/3.14 matrix running `build/python_ci.py` on
  push/PR to `main` touching `python/**`. There is **no publish job anywhere in CI**; PyPI
  publishing has always been the manual `cd python/micromegas && poetry build && poetry publish`
  step (Phase 2 of every `tasks/completed/release_v0.*_plan.md`). **No workflow change needed**, and
  the throw-away branch is not pushed, so `python.yml` never fires for it.
- **Tags** — every existing tag is either a `cargo release` crate tag (`micromegas-*-v0.31.0`), or
  a `grafana-v*` / `capi-v*` / `blender-v*` platform tag. There is **no python tag convention** and
  **no workflow triggers on `python-v*`** (only `blender-v*` and `capi-v*` have tag triggers), so
  introducing `python-v0.31.1` fires nothing.
- **`build/python_ci.py`** — reusable as-is; it's the hermetic pytest subset plus `black --check`.

### Version plumbing

The package carries no hardcoded version string. `micromegas/cli/version.py::package_version()`
reads `importlib.metadata.version("micromegas")`, `__init__.py` sets `__version__` from it, and
`tests/cli/test_version.py` compares the two rather than asserting a literal. `pyproject.toml`'s
`version` field is therefore the single source, and `python/micromegas/dist` is gitignored
(`.gitignore:30`).

PyPI's current latest is **0.31.0**; 0.32.0 has never been published, so 0.31.1 becomes latest and
is superseded normally when 0.32.0 ships.

## Design

Three separable pieces of work, deliberately kept apart because they have different lifetimes:

```
main (aaeee7a6c, python 0.32.0-dev)
  │
  ├── release-pypi-0.31.1          [THROW-AWAY: deleted after publish]
  │     ├─ commit A: CHANGELOG — record v0.31.1
  │     └─ commit B: pyproject version 0.32.0 → 0.31.1   ← built & published from here
  │                                                        ← tagged python-v0.31.1
  │
  └── changelog-python-0.31.1      [PR to main: commit A only]
```

**Commit B never reaches `main`.** That is the whole mechanism for "0.31.1 on PyPI without
disturbing main's 0.32.0": the version is a single tracked file, edited on a branch that is
discarded. No `[tool.poetry]` dynamic versioning, no build-time override, no `poetry version`
invocation on `main`.

**The `python-v0.31.1` tag is what keeps the published tree recoverable** once the branch is
deleted — it pins commit B, which is otherwise unreachable. Without it, the exact source of a
published artifact would exist only inside the PyPI sdist.

**Commit A is the only thing `main` needs**, because `main`'s `## Unreleased` section currently
holds the #1590 and #1588 bullets for these two fixes. Left there, the eventual v0.32.0 section
claims them as new and no record exists that a 0.31.1 was ever published. Commit A inserts a
`## v0.31.1 - 2026-09-16 (Python client only)` heading below `## Unreleased` (line 5) and moves
those two bullets into it verbatim, with a one-line preamble naming the version discontinuity (cut
from `main`'s python tree while the repo read 0.32.0). Carrying it to `main` as its own PR keeps the
version bump out of `main` while the record lands through the normal review path.

## Implementation Steps

### Phase 1: Pre-flight verification (autonomous)

1. Confirm the branch is `release-pypi-0.31.1` off `aaeee7a6c` and the tree is clean.
2. Re-confirm the shipping diff is only the three files listed in Current State:
   `git diff --stat micromegas-v0.31.0..HEAD -- python/`
3. Confirm PyPI has no 0.31.1 (a published version can never be replaced, only yanked):
   `curl -s https://pypi.org/pypi/micromegas/json | python3 -c "import sys,json; print(json.load(sys.stdin)['info']['version'])"`
4. From the repo root: `python3 build/python_ci.py` — hermetic pytest subset + `black --check`.

### Phase 2: CHANGELOG record — commit A (autonomous)

5. Edit `CHANGELOG.md`: insert `## v0.31.1 - 2026-09-16 (Python client only)` after the
   `## Unreleased` block's remaining entries, and move the two `**Python:**` bullets (#1590, #1593's
   entry, and #1588, #1591's entry) out of `## Unreleased` into it. The **Build** (toolchain) and
   **Packaging** (crate description) bullets stay in `## Unreleased` — neither ships in a python
   wheel. Target `## Unreleased` by line number (5); the literal string appears in body text
   elsewhere.
6. Commit A: `git commit -m "Record Python client v0.31.1 in CHANGELOG"`.

### Phase 3: Version bump and build — commit B (autonomous)

7. `python/micromegas/pyproject.toml` line 3: `version = "0.32.0"` → `version = "0.31.1"`. This is
   the only file edited; nothing else in the repo reads it.
8. `cd python/micromegas && poetry install` — rewrites the venv's `dist-info` so
   `importlib.metadata.version("micromegas")` reports 0.31.1 rather than the stale 0.32.0.
9. Commit B: `git commit -m "Set Python package version to 0.31.1 for PyPI patch release"`.
10. `git tag python-v0.31.1` (local; the push is gated in Phase 5).
11. `rm -rf python/micromegas/dist && poetry build` — a stale `dist/` from a previous cycle would
    otherwise be re-uploaded alongside the new files by `poetry publish`.

### Phase 4: Artifact verification before publish (autonomous)

Run every check here *before* Phase 5 — PyPI uploads are immutable.

12. `ls python/micromegas/dist/` → exactly `micromegas-0.31.1.tar.gz` and
    `micromegas-0.31.1-py3-none-any.whl`, nothing else.
13. Wheel metadata: `unzip -p dist/micromegas-0.31.1-py3-none-any.whl micromegas-0.31.1.dist-info/METADATA | head -20`
    → `Version: 0.31.1`, the current `Summary`, and the `Requires-Dist` set matching
    `pyproject.toml` (notably `cryptography>=50.0.0`).
14. Console scripts: `unzip -p dist/*.whl micromegas-0.31.1.dist-info/entry_points.txt` → all seven
    (`micromegas-grants`, `-groups`, `-import-keys`, `-logout`, `-query`, `-screens`,
    `-setup-telemetry`).
15. sdist contents: `tar tzf dist/micromegas-0.31.1.tar.gz | head -30` → the `micromegas/` package
    including `cli/setup_telemetry.py`, plus `README.md`.
16. Confirm the shipped fix is actually in the artifact:
    `unzip -p dist/*.whl micromegas/cli/setup_telemetry.py | grep -c "shlex.quote\|hasattr(os, \"fchmod\")"`
    → non-zero.

### Phase 5: Publish — HELD for explicit instruction

17. `cd python/micromegas && poetry publish` (requires `poetry config pypi-token.pypi <token>`).
18. `git push origin python-v0.31.1` — the tag only; the branch is never pushed.

### Phase 6: Post-publish verification

19. `curl -s https://pypi.org/pypi/micromegas/json | python3 -c "..."` → latest is 0.31.1.
20. Fresh-venv smoke install, outside the repo so nothing resolves from the working tree:
    ```bash
    python3 -m venv /tmp/mm-0311 && /tmp/mm-0311/bin/pip install micromegas==0.31.1
    /tmp/mm-0311/bin/micromegas-query --version          # → micromegas-query 0.31.1 (Python ...)
    /tmp/mm-0311/bin/micromegas-setup-telemetry --help   # → lists --format {posix,powershell,cmd,dotenv}
    ```

### Phase 7: Land the CHANGELOG record on main — PR creation HELD for explicit instruction

21. `git checkout -b changelog-python-0.31.1 main && git cherry-pick <commit A>`.
22. `git log --oneline main..HEAD` → exactly one commit, touching only `CHANGELOG.md`.
23. Push and open the PR.

### Phase 8: Cleanup

24. After the PR merges: `git branch -D release-pypi-0.31.1`. The `python-v0.31.1` tag keeps
    commit B reachable.
25. Move this plan to `tasks/completed/release_python_v0.31.1_plan.md`.

## Files to Modify

Throw-away branch only:
- `python/micromegas/pyproject.toml` — version 0.32.0 → 0.31.1 (commit B, never merged)

Landed on `main` via PR:
- `CHANGELOG.md` — new `## v0.31.1` section, two bullets moved out of `## Unreleased` (commit A)

Explicitly **not** touched: `build/release.py`, `.github/workflows/python.yml`, `rust/Cargo.toml`,
`grafana/package.json`, `analytics-web-app/package.json`,
`blender/micromegas_blender/blender_manifest.toml`, `README.md`.

## Trade-offs

**Branch off `main` HEAD vs. cherry-pick onto the `micromegas-v0.31.0` tag.** Cherry-picking onto
the tag is the textbook patch-release shape and would isolate the release from #1585/#1586/#1587.
Rejected because the diff makes it pointless: those three commits touch no python file, so the two
trees' `python/` directories are byte-identical apart from the two fixes. Branching off HEAD gets
the same artifact with no conflict risk and no second base to reason about.

**Edit `pyproject.toml` on a discarded branch vs. a dynamic/injected version.** A build-time
override (`poetry version 0.31.1` in a pre-publish script, or a dynamic-version plugin) would avoid
the discarded commit, but it adds permanent machinery to the repo for a one-off and leaves the
published version recorded nowhere in git. The discarded commit plus a tag is less clever and more
auditable.

**A `python-v0.31.1` tag vs. no tag.** No python tag convention exists, so this invents one for a
single release. Taken anyway because it is the only thing keeping the published tree reachable after
the branch is deleted, it costs one ref, and no workflow triggers on the pattern.

**CHANGELOG record as a separate PR vs. not recording it at all.** The branch being throw-away
argues for skipping it. Rejected: without it, the two bullets stay in `## Unreleased` and v0.32.0
re-announces already-published fixes, and nothing in the repo would show that 0.31.1 exists.

## Decisions

- No `README.md` "Recent Releases" entry — that section lists platform releases (crates, images,
  plugin); a python-only patch isn't one.
- No `gh release create` and no `v0.31.1` platform tag — a GitHub release for a python-only patch
  would imply crates and images shipped at that version, which they did not.
- The `## Unreleased` **Build** and **Packaging** bullets stay put; neither is in a python wheel.
- Not re-verifying #1591's Windows fix as part of this release. It needs a real `python.exe` (the
  crash is `os.fchmod` missing before Python 3.13, unreachable from WSL or CI, which is
  Linux-only); the fix was verified when it landed, and re-verification isn't a release gate.
- No update to `tasks/release_plan_template.md` — it covers the full platform release cycle, and
  folding an out-of-band python patch into it would misrepresent an exception as routine.

## Documentation

None. No user-facing documentation changes: the two shipping commits already landed their own
`mkdocs/` updates on `main`, and neither `mkdocs/` nor `python/micromegas/README.md` pins a client
version.

## Testing Strategy

`build/python_ci.py` (Phase 1, step 4) is the whole automated gate, and it already covers the
shipping code: `tests/cli/` is in its hermetic list, and #1591/#1593 added 394 lines to
`tests/cli/test_setup_telemetry.py` — the four format dialects' quoting, the endpoint validation
that runs before the key is minted, the `fchmod`-absent path, and the binary-mode write that keeps
`\n` from becoming `\r\n`. No new test is warranted: this release adds no code.

The integration suite is deliberately not run. It needs a live service, and at 0.31.0 it reported 82
`FlightUnavailable`/`ConnectionError` failures without one — it would test `main`'s server behavior,
which this release does not ship.

The artifact checks in Phase 4 are the tier unit tests genuinely cannot reach: whether
`poetry build` put the right version, dependency set, and seven console-script entry points into a
zip. A wrong wheel is also the one failure here that cannot be corrected after the fact, since a
published PyPI version is immutable.

## Manual Verification

The fresh-venv install in Phase 6, step 20. It is the only check that exercises the real artifact
resolved from the real index — `--version` printing `0.31.1` confirms the wheel's metadata survived
the upload, and `--help` listing `--format` confirms the shipped module is the post-#1593 one and
not a stale build. Automating it would mean a CI publish pipeline this repo has never had, for a
release type that is by definition exceptional.

## Risks

- **PyPI immutability** — 0.31.1 cannot be re-uploaded, only yanked and superseded by 0.31.2. This
  is why every verification step precedes Phase 5.
- **Commit B leaking into `main`** — would silently regress `main`'s version to 0.31.1 and make the
  next release try to publish a version PyPI already has. Guarded by step 22's
  `git log --oneline main..HEAD` one-commit check on the PR branch.
- **Stale `dist/`** — `poetry publish` uploads everything in `dist/`, so a leftover 0.32.0-dev
  artifact from a prior local build would be published too. Step 11 removes the directory first.

## Autonomy Boundaries

Per `CLAUDE.md`, nothing is pushed or published without a direct instruction. **Autonomous**:
Phases 1–4 and 8 — verification, CI, the CHANGELOG and version edits, local commits, the local tag,
and `poetry build`. **Held for explicit approval**: `poetry publish`, `git push origin
python-v0.31.1`, and pushing/opening the Phase 7 PR.
