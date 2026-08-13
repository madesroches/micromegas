# CLAUDE.md — unreal/

This folder mirrors Unreal Engine source that normally lives in a private
Perforce workspace: `MicromegasTracing` (from `Engine/Source/Runtime/Core`)
and `MicromegasTelemetrySink` (from a game/plugin module). Perforce is the
place these files are actually edited day-to-day; this git repo is the public
mirror.

## Perforce → git (pulling changes in)

1. Set two env vars pointing at the local P4 workspace:
   - `MICROMEGAS_UNREAL_ROOT_DIR` — root of the Unreal Engine source tree
     (e.g. `F:\p4\UE`)
   - `MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR` — directory containing the
     `MicromegasTelemetrySink` module (may be inside a plugin folder)
2. From the repo root, run:
   ```
   python3 build/copy_unreal_from_workspace.py
   ```
   This copies the three source directories from the P4 workspace into
   `unreal/`, converting CRLF → LF for known text extensions. It refuses to
   overwrite a destination that has untracked or locally modified files —
   commit or stash those first.
3. Review `git diff unreal/` before committing:
   - Check for stray encoding artifacts (e.g. a UTF-8 BOM added by an editor)
     on files that shouldn't have actually changed — strip these, they're
     noise, not real edits.
   - Scan for anything that shouldn't leave the internal Perforce tree:
     internal project/codenames, absolute workspace paths, usernames,
     internal URLs, credentials/tokens. None of that belongs in the public
     repo — if you find any, edit it out (or ask before committing) rather
     than publishing it.
4. Commit only the real, reviewed changes.

`build/unreal_hard_link_windows.py` is deprecated (it used junctions instead
of copying) — don't use it.

## git → Perforce (pushing changes back out)

There's no script for this direction; it's manual:

1. In the P4 workspace, check out for edit (or add) the corresponding files
   under `$MICROMEGAS_UNREAL_ROOT_DIR/Engine/Source/Runtime/Core/{Public,Private}/MicromegasTracing`
   and `$MICROMEGAS_UNREAL_TELEMETRY_MODULE_DIR/MicromegasTelemetrySink`.
2. Copy the changed files from `unreal/` over them, matching Perforce's own
   line-ending convention for that tree (don't fight the P4 typemap).
3. Review the P4 diff, then submit with a changelist description that
   references the git commit(s) being ported.

## General

- Both directories carry their own `README.md` noting they're mirrored from
  https://github.com/madesroches/micromegas/ and require authorization
  before contributing changes — leave those in place.
- Treat this as a one-way-tooled, two-way-maintained mirror: automate
  Perforce → git with the copy script, but review every sync for leaked
  internal details before it's committed, since the source of truth lives
  in a private, internal tree.
