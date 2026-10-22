# build_docker_images.py: Fail Loudly on Build/Push Failure Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1699

## Overview
During a release, 3 of 8 services failed to build from transient network errors and the script
ended with a `Total: 10/16 built, 10/16 pushed` line that is easy to miss after a ~2 hour run.
Make failures unmistakable: exit non-zero whenever any service/arch failed to build **or push**,
and print the failed services plus the exact command to re-run only those. Automatic retries are
out of scope (a retry loop can spin on a real failure; the buildx layer cache makes a manual
re-run incremental).

## Current State
`build/build_docker_images.py`:
- `build_image` returns a dict with `built` and `pushed`. For amd64 `--push`, a failed
  `docker push` returns with `built=True, pushed=False`.
- `main` computes `failed = [r for r in results if not r["built"]]` and returns 1 only if that is
  non-empty. **A push failure after a successful build therefore exits 0.**
- The `Failed:` list prints only the service name — no architecture, so under `--all-arches` it
  is ambiguous which image failed — and `build_image` does not record the arch in its result.
- No re-run command is printed.
- Existing tests: `build/test_build_docker_images.py` (monkeypatched `run_command`).

## Design
- `build_image` adds an `"arm64"` key to its result dict.
- Add a pure helper `failed_results(results, push)`: a result failed if `not built`, or if `push`
  and `not pushed`.
- Add a pure helper `rerun_command(failed, args)` returning the command string for re-running
  only the failed images. Because arches are selected by flags, group failed results by arch:
  - only amd64 failures: `python3 build/build_docker_images.py [--push] [--version V] svc...`
  - only arm64 failures: same with `--arm64`
  - both: one line per arch (`--all-arches` cannot target different service sets per arch).
  `--push` and an explicit `--version` (when the user passed one) are carried over.
- `main` replaces the `failed` computation with `failed_results`, prints each failure as
  `service (amd64|arm64): not built | not pushed`, prints the re-run command(s) under a
  `Re-run only the failed images:` heading, and returns 1.
- Summary header on failure is a clear `FAILED: N image(s)` line placed at the end of the output
  so it is the last thing on screen.
- Unknown-service early return in `build_image` keeps `built=False`, so it is counted as failed;
  `main` already validates names up front so this is unreachable in practice.

## Implementation Steps
1. Add `arm64` to the `build_image` result dict.
2. Add `failed_results` and `rerun_command` in `build/build_docker_images.py`.
3. Update `main` summary: use `failed_results`, print reasons and re-run command, return 1.
4. Add unit tests in `build/test_build_docker_images.py`.
5. CHANGELOG entry.

## Files to Modify
- `build/build_docker_images.py`
- `build/test_build_docker_images.py`
- `CHANGELOG.md`

## Trade-offs
- Automatic retries: rejected per the issue (can loop on real failures; re-run is cheap).
- Issue follow-ups (Dockerfile `CARGO_NET_RETRY`/`CARGO_HTTP_TIMEOUT`, skipping already-published
  tags, final tag inspection) are separate work and not included.

## Documentation
No docs reference the failure behavior. `tasks/release_plan_template.md` mentions the script but
needs no change.

## Testing Strategy
No-Docker unit tests (monkeypatched `run_command`, as existing tests do):
- `failed_results`: all ok → empty; build failure → included; amd64 push failure (built, not
  pushed) → included only when `push=True`; not flagged without push.
- `rerun_command`: amd64-only, arm64-only, mixed arches (two lines), `--push` carried over,
  `--version` carried over only when given.
- `build_image` result carries the `arm64` flag.
- `main` exit code: with `build_image` monkeypatched, a push-failure result yields exit 1 and the
  output contains the re-run command; an all-success run yields 0.
