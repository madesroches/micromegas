# `micromegas-setup-telemetry --env-file` on Windows Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1588

## Overview

On Windows, `micromegas-setup-telemetry --env-file PATH` mints an ingestion API key
server-side and then raises `AttributeError: module 'os' has no attribute 'fchmod'`
before writing a single byte. The target file has already been created and truncated
by that point, so the user is left with a 0-byte file and a key that was never printed
anywhere — unrecoverable, and re-minting leaves a live orphaned key only an admin can
revoke. This plan makes `write_env_file` platform-portable, moves its destructive step
after the step that can fail, and widens `run()`'s key-preservation fallback so it
actually catches the class of failure it was written to catch.

## Current State

`python/micromegas/micromegas/cli/setup_telemetry.py`

`write_env_file` (`python/micromegas/micromegas/cli/setup_telemetry.py:244`):

```python
fd = os.open(str(target), os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
try:
    os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)
    os.write(fd, content.encode("utf-8"))
finally:
    os.close(fd)
```

`os.open` creates-and-truncates; `os.fchmod` is documented "Availability: Unix" and
does not exist as an attribute at all on Windows, so it raises `AttributeError`; and
`os.write` never runs. The single destructive step precedes the only step that cannot
work on the platform.

`run()`'s `--env-file` branch (`python/micromegas/micromegas/cli/setup_telemetry.py:392`)
already carries a fallback built for exactly this hazard — it prints the exports to
stdout, warns on stderr, and re-raises, with a comment stating the invariant ("The key
was already minted above and is never retrievable again … must never discard it"). But
it is `except OSError`, and `AttributeError` is not an `OSError`, so the fallback never
fires.

`main()` (`python/micromegas/micromegas/cli/setup_telemetry.py:410`) catches
`(RuntimeError, requests.exceptions.RequestException, config.ProfileError, OSError)`
to render `Error: <msg>` and `sys.exit(1)`; anything else escapes as a traceback.

Related precedent: `OidcAuthProvider.save()`
(`python/micromegas/micromegas/auth/oidc.py:474`) uses the same
`os.open(..., O_CREAT | O_WRONLY | O_TRUNC, 0o600)` idiom for the token cache but has
**no** `fchmod` call, which is why it works on Windows. `write_env_file`'s docstring
claims to mirror that function's permissions; the `fchmod` is the divergence.
`tests/auth/test_oidc_unit.py:73` already establishes the repo's precedent for a
mode assertion that is skipped on Windows.

Docs: `mkdocs/docs/query-guide/python-api.md:1104` states flatly that `--env-file PATH`
"writes the exports to a `0o600` file … (parent directory created `0o700` if needed)",
with no platform caveat.

CI (`.github/workflows/python.yml`) runs `ubuntu-latest` only, on Python 3.11 and 3.14,
via `build/python_ci.py` → `poetry run pytest` over an explicit hermetic file list that
includes `tests/cli`, plus `black --check .`. There is no Windows job, so the regression
has to be pinned by simulating the platform rather than running on it.

## Design

Three changes, each independently sufficient to prevent the key loss, applied together
because they fix different layers.

### 1. `write_env_file`: write first, harden second, and guard the Unix-only call

```python
fd = os.open(str(target), os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
try:
    # Written before the mode is re-asserted below: `os.open` already
    # create-and-truncated `target`, so a hardening failure after this point costs
    # nothing, while one before it would leave the file empty and the just-minted,
    # never-retrievable key with nowhere to land.
    os.write(fd, content.encode("utf-8"))
    # `os.fchmod` is Unix-only. `os.open`'s mode argument is masked by the umask,
    # which can only clear bits, so this only ever restores an owner bit the umask
    # stripped -- it can never widen the file past `0o600`.
    if hasattr(os, "fchmod"):
        os.fchmod(fd, stat.S_IRUSR | stat.S_IWUSR)
finally:
    os.close(fd)
```

`hasattr` rather than a `sys.platform` test: the availability of the syscall is the
actual precondition, and it is also what a test can manipulate with
`monkeypatch.delattr`.

A hardening failure is still allowed to propagate (it is not swallowed): a secret
sitting at a mode we could not restrict is worth surfacing, and `run()`'s fallback —
broadened below — now guarantees the key survives the raise.

### 2. `run()`: broaden the key-preservation net to `except Exception`

The invariant being defended is "a minted key must never be discarded", which argues
for catching everything rather than enumerating the failure types we happened to think
of. `except OSError` → `except Exception`, keeping the existing warn-to-stderr /
exports-to-stdout / re-raise body verbatim.

`BaseException` is deliberately not used — see Decisions.

### 3. Docstring and docs: state the Windows permission caveat instead of over-promising

`write_env_file`'s docstring currently promises mode `0o600` unconditionally. On
Windows, POSIX mode bits are not enforced: `os.fchmod` does not exist, and `os.chmod`
there honors only the read-only bit. Genuinely restricting the file to its owner needs
an ACL change, which is out of scope for this CLI — so the file lands at the directory's
inherited ACL and that should be documented rather than crashed on. The same caveat goes
into `mkdocs/docs/query-guide/python-api.md`'s `--env-file` sentence.

## Implementation Steps

1. **`python/micromegas/micromegas/cli/setup_telemetry.py` — `write_env_file`**
   - Swap the `os.write` and `os.fchmod` statements, and wrap the `os.fchmod` call in
     `if hasattr(os, "fchmod"):`.
   - Replace the existing "Belt-and-suspenders" comment with the two comments shown in
     Design §1 (why the write comes first; why the guard exists and why it cannot widen
     the mode).
   - Update the docstring: mode `0o600` "where the platform enforces it", plus one
     sentence on Windows landing at the inherited ACL because restricting it needs an
     ACL change this CLI does not make.

2. **`python/micromegas/micromegas/cli/setup_telemetry.py` — `run()`**
   - `except OSError as e:` → `except Exception as e:` in the `--env-file` branch. Body
     unchanged.
   - Extend the existing comment's parenthetical list of causes with "a platform-missing
     syscall" so the reason the net is this wide is recorded where it is widened.

3. **`python/micromegas/tests/cli/test_setup_telemetry.py`** — see Testing Strategy for
   the four tests.

4. **`mkdocs/docs/query-guide/python-api.md`** — amend the `--env-file PATH` sentence
   (line ~1104) with the Windows caveat.

5. **`CHANGELOG.md`** — one `* **Python:**` bullet under `## Unreleased` describing the
   crash, the key loss it caused, and all three fixes.

## Files to Modify

- `python/micromegas/micromegas/cli/setup_telemetry.py`
- `python/micromegas/tests/cli/test_setup_telemetry.py`
- `mkdocs/docs/query-guide/python-api.md`
- `CHANGELOG.md`

## Trade-offs

- **Considered: an atomic write (sibling temp file via `tempfile.mkstemp`, then
  `os.replace`).** This would eliminate truncation entirely — the target is never
  touched until a complete, already-hardened file is ready to swap in — and would also
  close the pre-existing-file mode window noted in Decisions. Rejected as heavier than
  the exposure warrants: with fixes 1 and 2 in place the minted key is preserved on
  every failure path, the only remaining casualty of a truncation is a *previous* key's
  env file that the user is in the middle of replacing anyway, and the temp-file variant
  adds failure-path cleanup and a stale-temp-file mode of its own. Recorded here so the
  option is not re-derived from scratch if truncation ever becomes a real complaint.
- **Considered: `sys.platform != "win32"` instead of `hasattr(os, "fchmod")`.** The
  attribute's presence is the actual precondition for the call, is what CPython's own
  docs describe as the portability contract, and is directly manipulable from a test on
  a Linux runner. A platform string would need a second mechanism to be testable.
- **Considered: broadening `main()`'s `except` tuple too.** Left alone — see Decisions.

## Decisions

- **Scope excludes the issue's "Related nit" (`--format posix/powershell/cmd/dotenv`).**
  The reporter explicitly offered to split it out, and it is an additive feature with its
  own flag, output matrix, docs and tests, not part of fixing a crash that loses keys.
  Recommend filing it separately; this plan does not implement it.
- **`main()`'s `except` tuple stays as-is** (no `Exception` there). `run()`'s wide net
  exists to protect a specific, irreplaceable value; `main()`'s narrow tuple exists to
  render *expected* errors as `Error: <msg>`. A genuinely unexpected exception reaching
  `main()` should still print a traceback, because that is a bug report, not a user
  error. After fix 1 the `AttributeError` path no longer exists to reach it.
- **`except Exception`, not `except BaseException`.** A `KeyboardInterrupt` landing
  inside a sub-millisecond local file write is not a realistic key-loss path, and having
  Ctrl-C print a secret to stdout is more surprising than the risk it averts.
- **Accepted risk: the one-syscall window where a *pre-existing* target holds the key at
  its old mode.** `os.open` does not apply its mode argument to a file that already
  exists, so writing before hardening means a target the user had previously left at,
  say, `0o644` holds the key for the duration of one `fchmod` call. A newly created file
  is never affected — `os.open`'s mode is umask-masked, so it can only be at or below
  `0o600`. Accepted: the looser mode is the caller's own doing, inside a directory this
  function creates at `0o700` whenever it creates it, and closing the window costs the
  atomic-write design rejected above.
- **A hardening failure is not swallowed.** Propagating it means the user learns their
  credential file may not be restricted; fix 2 is what makes propagating safe.

## Documentation

- `mkdocs/docs/query-guide/python-api.md` — the `--env-file PATH` sentence in the
  `micromegas-setup-telemetry` section gains the Windows caveat (mode bits are not
  enforced there; the file lands at the inherited ACL).
- `CHANGELOG.md` — `## Unreleased`, one `* **Python:**` bullet.
- No new docs page; no `mkdocs.yml` nav change.

## Testing Strategy

All four tests are plain unit tests in
`python/micromegas/tests/cli/test_setup_telemetry.py`, collected by the existing
hermetic list (`tests/cli` is already in `build/python_ci.py`'s `HERMETIC_TEST_ARGS`).
No live DB or service is involved, and none is warranted: every behavior here is
reachable by calling the two functions directly.

1. **`test_write_env_file_writes_content_when_fchmod_is_unavailable`** — the regression
   test for the bug witnessed in the wild. `monkeypatch.delattr(os, "fchmod",
   raising=False)` simulates Windows on the Linux runner, then `write_env_file(tmp_path /
   "sub" / "telemetry.env", content)` must return normally and the file must contain
   `content`. Pins both the `AttributeError` crash and the 0-byte file. Simulating the
   platform is the only option available — CI has no Windows job, and adding one for a
   single `hasattr` branch is not worth a second matrix leg.
2. **`test_write_env_file_writes_content_before_hardening_permissions`** — pins the
   ordering. `monkeypatch.setattr(os, "fchmod", raising_fchmod)` where the stub raises
   `PermissionError`; `write_env_file` must raise, *and* the target must already hold the
   full `content`. Without the reorder this test fails with an empty file.
3. **`test_run_env_file_write_failure_prints_key_to_stdout_and_reraises_non_oserror`** —
   the regression test for the safety net. Mirrors the existing
   `test_run_env_file_write_failure_prints_key_to_stdout_and_reraises` (line 667) but
   with `write_env_file` monkeypatched to raise `AttributeError("module 'os' has no
   attribute 'fchmod'")`. Asserts `Authorization=Bearer mmk_secret` on stdout, a warning
   naming the path on stderr, and `pytest.raises(AttributeError)`.
4. **Amend `test_run_writes_env_file_with_secure_permissions_and_prints_its_path`**
   (line 635) — guard its `mode == 0o600` assertion on `hasattr(os, "fchmod")` so the
   suite is honest about what the code now promises, following the
   `tests/auth/test_oidc_unit.py:73` precedent. The content and printed-path assertions
   stay unconditional.

Run: `cd python/micromegas && poetry run pytest tests/cli/test_setup_telemetry.py`, then
`python build/python_ci.py 3.11` from the repo root for the full hermetic suite plus the
`black --check` gate.

## Manual Verification

Only the `hasattr` branch's behavior on a real Windows host is beyond the unit tests
(they simulate the missing attribute rather than running without it), and the failure
mode there is immediately obvious to whoever runs it:

1. On a Windows host with the branch installed, run
   `micromegas-setup-telemetry --url <deployment> --name <machine> --otlp-endpoint
   <deployment>/ingestion/otlp --user-audience <suffix> --env-file
   %USERPROFILE%\.config\telemetry.env`.
   Expected: the `minted ingestion api key (...)` line on stderr, the env-file path on
   stdout, no traceback, and a `telemetry.env` containing all three
   `OTEL_EXPORTER_OTLP_*` export lines.
2. Re-run the same command. Expected: the file is rewritten with the new key, again with
   no traceback.

Not automated because it needs a Windows runner and a live deployment to mint against;
the platform-independent halves are both covered by tests 1–3 above. If no Windows host
is available, say so rather than reporting the step as done — tests 1–3 are what actually
gate the fix.
