# Docker Image Revision Label and Commit-SHA Tag Plan

Issue: #1621

## Overview

Images built by `build/build_docker_images.py` are tagged only `<cargo-version>` and `latest`
(`-arm64`-suffixed under `--arm64`). Nothing in the image records which commit it came from.
The Cargo version stays the same across all the commits between two releases, and `latest`
moves. So a deployment that runs or mirrors an unreleased build can't give every service from
one commit a matching tag, and can't tell which commit a running image was built from. This
plan makes the script compute the source revision once per run. Every image then gets the
standard OCI label `org.opencontainers.image.revision=<full-sha>[-dirty]` and a third tag,
`<sha12>[-dirty][-arm64]`, next to the existing two.

## Current State

`build/build_docker_images.py`:

- `main()` (`:187`) resolves `version` (`--version` or `get_version()` from `rust/Cargo.toml`),
  then calls `build_image(service, version, push, arm64)` for each service × arch (`:237-241`).
- `build_image()` (`:72-184`) computes `version_tag`/`latest_tag` (`:100-107`) and puts them in
  `result["tags"]`. It then assembles one of three near-identical command lists by hand:
  - arm64 + push: `docker buildx build --platform linux/arm64 --push … -t v -t latest .` (`:117-131`)
  - arm64 local: the same with `--load` (`:138-152`)
  - amd64: `docker build … -t v -t latest .` (`:158-168`), followed by two hard-coded
    `docker push` calls, one per tag (`:176-182`)
- The summary (`:244-270`) already iterates `r["tags"]`, so a new tag shows up there for free.
- The script has no tests. Its sibling `build/test_rust_ci.py` shows the local pattern: pytest,
  importing functions straight from the script module, with no external tools.

The tag scheme is documented in `docker/README.md` under "Tag scheme" (`:22-27`).

## Design

### Revision

A new function computes the revision once, in `main()`, and passes it to every `build_image`
call:

```python
def get_revision(cwd: Path = REPO_ROOT) -> str:
    """`<full-sha>` of HEAD, with `-dirty` appended when the worktree has changes."""
```

- `git rev-parse HEAD` → full 40-char sha.
- `git status --porcelain` non-empty → append `-dirty`. Untracked files count as changes.
  That's intended: unless `.dockerignore` excludes them, untracked files are part of the build
  context.
- Use `subprocess.run(..., cwd=cwd, capture_output=True, text=True, check=True)`. If git is
  missing or the directory is not a checkout, the script prints a clear error and exits
  non-zero. It never falls back to an unlabeled build (see Decisions).
- `cwd` is a parameter so the unit tests can point it at a temporary repo.

`main()` prints `Revision: <revision>` next to the existing `Version:` line, both at the start
of the run and in the BUILD SUMMARY. The revision is taken once at startup, so if the tree
changes during a long `--all-arches` run, every image still carries the same tag.

### Tags

Tag computation moves into a pure function:

```python
def image_tags(version: str, revision: str, arm64: bool) -> list[str]:
    # -> [version, "latest", revision_tag] with "-arm64" appended to each when arm64
```

The revision tag is `revision[:12]`, plus `-dirty` when `revision.endswith("-dirty")`, plus
`-arm64` on arm64. Examples: `6a6822cb3f1e`, `6a6822cb3f1e-dirty`, `6a6822cb3f1e-dirty-arm64`.
Every character is in Docker's tag charset `[A-Za-z0-9_.-]`, and the longest form is well under
the 128-char limit. The order stays `[version, latest, sha]`, so the existing tags keep their
position in `result["tags"]` and in the summary.

### Command assembly (DRY)

The three hand-built command lists become one pure builder:

```python
def build_command(dockerfile: str, image_name: str, tags: list[str],
                  revision: str, arm64: bool, push: bool) -> list[str]:
```

- Prefix: `["docker", "build"]` for amd64, and
  `["docker", "buildx", "build", "--platform", "linux/arm64", "--push" if push else "--load"]`
  for arm64.
- Then `-f <DOCKER_DIR/dockerfile>`, one `-t image:tag` per tag,
  `--label org.opencontainers.image.revision=<revision>`, and `.`.

`build_image(service, version, revision, push, arm64)` calls `image_tags` and then
`build_command`, and runs the result. After a successful amd64 build with `push=True`, it runs
`docker push image:tag` once for each entry in `result["tags"]`, replacing the two hard-coded
pushes, so the sha tag is pushed as well. The arm64 path is unchanged apart from the new
arguments: `--push` still sets both `built` and `pushed`.

The label is applied through `--label` on the command line, which sets it on the final image
config.

A consumer reads the label with:

```
docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' <image>
```

## Implementation Steps

1. **`build/build_docker_images.py`**
   - Add `get_revision()`, `image_tags()` and `build_command()` as specified above.
   - Change `build_image()` to take `revision`, derive its tags from `image_tags()`, build its
     command with `build_command()`, and push by looping over `result["tags"]`.
   - In `main()`, compute `revision = get_revision()` after `version` (catch
     `CalledProcessError`/`FileNotFoundError` → print the error and return 1). Print it, pass it
     to `build_image`, and print it in the summary.
   - Update the module docstring to mention the sha tag and the revision label.
2. **`build/test_build_docker_images.py`** (new): pytest unit tests, listed under Testing
   Strategy.
3. **`docker/README.md`**: add the sha tag to the "Tag scheme" table, and add a short note with
   the label name and the `docker image inspect` command.
4. **`CHANGELOG.md`**: add a `**Build:**` entry under Unreleased that references #1621.

## Files to Modify

- `build/build_docker_images.py`
- `build/test_build_docker_images.py` (new)
- `docker/README.md`
- `CHANGELOG.md`

## Trade-offs

- **CLI `--label` vs. a `LABEL` + `ARG` in each Dockerfile.** Putting the label in the
  Dockerfiles would mean editing all nine files, and an `ARG` whose value changes on every
  commit invalidates the cache from the layer that uses it onward. The CLI flag is a single
  change in one place and never affects the cache.
- **12-char sha vs. the full sha in the tag.** Twelve characters are unique in practice for a
  repo this size and keep tags readable. The label always has the full sha, so the exact commit
  can still be recovered.
- **Refactor into `build_command()` vs. adding `-t`/`--label` to each of the three lists.**
  Patching the three lists would triple both the edit and the risk of one path missing the
  label. The refactor also makes the commands unit-testable without Docker.
- **Per-tag `docker push` vs. `docker push --all-tags`.** `--all-tags` would also push any stale
  local tags of the repository, such as older sha tags. Pushing exactly `result["tags"]` is
  precise.

## Decisions

- If git is missing or the directory is not a checkout, the build fails loudly. The script
  always runs from a checkout, and silently skipping the label or tag would defeat the feature.
- A dirty tree does not block `--push`. The `-dirty` suffix on both the tag and the label is the
  signal. A `<sha12>-dirty` tag is not unique, because two different dirty states of the same
  commit share it, and a later push overwrites the earlier one. The suffix already tells the
  consumer the image isn't a reproducible build, so this is accepted.
- The new test file is run locally with `python3 -m pytest build/test_build_docker_images.py`.
  It is not wired into CI: no existing workflow triggers on `build/build_docker_images.py`, and
  the script is run by hand. A dedicated workflow for one rarely changed script would cost more
  than it catches.
- Other OCI labels (`source`, `version`, `created`) are out of scope. `created` in particular
  would make every image config unique for each build.

## Documentation

- `mkdocs/docs/development/build.md` only mentions `--arm64`. It needs no change.

## Testing Strategy

`build/test_build_docker_images.py` (pytest, no Docker). It imports from `build_docker_images`
the same way `test_rust_ci.py` imports from `rust_ci`.

- `image_tags`: clean amd64 → `[v, "latest", sha12]`, clean arm64 → all three with `-arm64`,
  dirty amd64 → `sha12-dirty`, dirty arm64 → `sha12-dirty-arm64`.
- `build_command`, for each of the three paths (amd64, arm64 `--load`, arm64 `--push`): exactly
  one `--label org.opencontainers.image.revision=<full revision>`, one `-t` per tag, the correct
  prefix/`--load`/`--push`, and `.` last.
- `get_revision` against a `git init` repo in `tmp_path`, with one commit and a local
  `user.name`/`user.email`: clean → equals `git rev-parse HEAD`; after modifying a tracked file →
  ends with `-dirty`; with only an untracked file → ends with `-dirty`; in a non-repo directory →
  raises `CalledProcessError`.
- The push loop in `build_image`: monkeypatch `run_command` to record calls, then assert that an
  amd64 `push=True` build issues one `docker push` for each of the three tags.

## Manual Verification

These steps run a real Docker build, which unit tests can't cover. A broken build would be
obvious straight away.

1. `python3 build/build_docker_images.py redis-exporter` (the fastest image). The summary lists
   three tags, including `…:<sha12>` (`-dirty` if the tree has changes).
2. `docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' marcantoinedesroches/micromegas-redis-exporter:<sha12>`
   prints the full sha, which matches `git rev-parse HEAD`.
3. `python3 build/build_docker_images.py redis-exporter --arm64`, then repeat the inspect on the
   `<sha12>-arm64` tag. This confirms the buildx `--load` path carries the label.
