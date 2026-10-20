# Monolith Docker Start Script Plan

## Overview
Add a Python script that runs the monolith from the pre-built `marcantoinedesroches/micromegas-monolith` image and prints the URLs to click, mirroring `analytics-web-app/start_analytics_web_docker.py`. Today the only ways to run the monolith are building from source (`start_services.py --monolith`) or hand-typing `docker compose` commands with no feedback on when it is ready or where to go.

## Current State
- `analytics-web-app/start_analytics_web_docker.py` wraps `docker run` for the web image: checks Docker, checks the image exists, frees the port, starts the container, handles Ctrl+C, and prints `Web App: http://localhost:<port><base_path>/` via a local `print_status` helper (colored, emoji-prefixed). Its `-d/--detach` flag switches between foreground and background.
- `local_test_env/ai_scripts/start_services.py --monolith` runs the source-built monolith and prints PIDs and log paths; `stop_services.py` stops it.
- `docker/docker-compose.monolith.yaml` is already self-contained (Postgres 16, inline `pg_init` config creating `micromegas_app`, file-backed object store on the `lake` volume, `--disable-auth`, ports 3000/9000/50051, `MICROMEGAS_BASE_PATH=/`). It needs Compose v2.23.1+.
- `build/docker_command.py` and `build/run_*_container.py` are build/release helpers, not a user-facing launcher.
- No script ties the compose file to a "start it and tell me where to click" experience. `docs/GETTING_STARTED.md` and `mkdocs/docs/getting-started.md` document raw compose commands.

## Design
New script `docker/start_monolith_docker.py` (beside `docker-compose.monolith.yaml`, which it drives and locates as a sibling file). It is a thin driver over the existing compose file, which stays the single source of truth for the stack's configuration (DRY — no second copy of env vars/ports in Python).

Behavior:
1. Verify `docker info` and `docker compose version` (error with the Compose v2.23.1+ requirement, mirroring `docker/docker-compose.monolith.yaml`).
2. Use a fixed compose project name (`micromegas-monolith`) so containers/volumes don't depend on the checkout directory name and `--down` finds them.
3. Check that ports 3000, 9000 and 50051 are free (skipping the check if the project is already running); fail with the offending port.
4. `docker compose -f <repo>/docker/docker-compose.monolith.yaml -p micromegas-monolith up -d` (with `--pull always` when `--pull` is given; default uses the local image if present, otherwise compose pulls).
5. Poll until ready: web app (`http://localhost:3000/`) responds, and ingestion `GET http://localhost:9000/ready` returns 200 (probe from the ready-probe work). Timeout ~90s; on timeout print `docker compose logs --tail 50 micromegas` and exit 1.
6. Print summary in the same `print_status` style:
   - Web App: `http://localhost:3000/`
   - Ingestion: `http://localhost:9000`
   - FlightSQL: `grpc://localhost:50051` plus a ready-to-copy `micromegas-query "SELECT ..."` hint
   - Logs / stop commands (`docker compose ... logs -f`, `--down`)
7. Default mode is detached (the compose stack outlives the script). `--follow` tails logs after the summary and stops the stack on Ctrl+C, matching the foreground behavior of the web-app script.

Flags: `--pull`, `--follow`, `--down` (stop and remove containers; `--down --volumes` wipes data), `--timeout`. No port/image flags: the compose file hardcodes them, and adding overrides would need compose-file interpolation for little benefit — add later if wanted (open/closed: compose file can gain `${VAR:-default}` without changing the script's contract).

`print_status`, `check_docker`, and `check_port_in_use` are duplicated in `start_analytics_web_docker.py`. The web-app script lives in a different tree and is left untouched; the new script defines its own small helpers. A shared module is a follow-up if a third script appears.

Pure helpers (`parse_args`, `build_compose_cmd`, `format_summary`, `wait_for_ready` with an injectable probe) are separate functions so they can be unit tested without Docker.

## Implementation Steps
1. Create branch `monolith_docker_script` (done).
2. Write `docker/start_monolith_docker.py` per Design.
3. Add unit tests (see Testing Strategy).
4. Update docs (see Documentation) and `CHANGELOG.md` entry.

## Files to Modify
- `docker/start_monolith_docker.py` (new)
- `docker/test_start_monolith_docker.py` (new)
- `mkdocs/docs/getting-started.md`, `mkdocs/docs/development/build.md`, `docker/README.md` (mention)
- `CHANGELOG.md`

## Trade-offs
- **Wrap compose vs. `docker run` the monolith image.** `docker run` would need a separate Postgres container, network, and the `micromegas_app` DB bootstrap, duplicating the compose file. Wrapping compose reuses it unchanged.
- **Location.** `docker/` holds the compose file the script wraps, so the two stay together. `local_test_env/ai_scripts/` (next to `start_services.py --monolith`) was considered and rejected.
- **Detached by default** (differs from the web-app script's foreground default) because compose manages multiple containers and the user typically wants their terminal back after seeing the URLs; `--follow` gives the foreground behavior.

## Decisions
- Compose file is not modified in this change.
- Script lives in `docker/`, beside the compose file it wraps.

## Documentation
- `mkdocs/docs/getting-started.md`: mention the script as a convenience over raw `docker compose up` for repo checkouts (curl-only path stays compose-based).
- `mkdocs/docs/development/build.md` near the `start_services.py --monolith` section: add the Docker variant.
- `docker/README.md`: one line pointing at the script.
- `CHANGELOG.md`: new script entry.

## Testing Strategy
No-DB unit tests (pytest, run with the python poetry venv) for the pure helpers: argument parsing, compose command construction (`--pull`, `--down`, `--volumes`, project name, file path resolution from the script location), summary formatting (URLs/ports), and `wait_for_ready` with a fake probe covering success, eventual success, and timeout. No live Docker test: nothing here is a witnessed bug, and breakage in the wiring is obvious on first run.

## Manual Verification
Not automatable without Docker in CI; failure is immediately visible.
1. `python3 docker/start_monolith_docker.py` — expect the summary with `http://localhost:3000/`; open it and see the web app.
2. `micromegas-query "SELECT count(*) FROM log_entries" --begin 1h` — expect rows (self-telemetry) after a few seconds.
3. Re-run the script while running — expect it to report already running, not a port conflict.
4. `python3 docker/start_monolith_docker.py --down` — expect containers removed; `docker ps` shows none.
5. Occupy port 3000 (`python3 -m http.server 3000`) and start — expect a clear port-in-use error.

## Open Questions
- Do we want `--port`/`--image` overrides now (requires compose interpolation)?
