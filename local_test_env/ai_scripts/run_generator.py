#!/usr/bin/env python3
"""Run the telemetry-generator against the local ingestion server.

Enables CPU (thread) tracing so the generator emits a `cpu`-tagged thread
stream in addition to logs/metrics. Without MICROMEGAS_ENABLE_CPU_TRACING the
thread and async span streams are never produced, which makes the
thread_spans / async_events integration tests fail with no data.
"""

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
RUST_DIR = REPO_ROOT / "rust"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tasks", type=int, default=5000, help="tasks to generate")
    parser.add_argument("--async-tasks", type=int, default=10)
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument(
        "--telemetry-url",
        default=os.environ.get("MICROMEGAS_TELEMETRY_URL", "http://127.0.0.1:9000"),
    )
    parser.add_argument(
        "--release", action="store_true", help="build/run the release binary"
    )
    args = parser.parse_args()

    env = os.environ.copy()
    env["MICROMEGAS_ENABLE_CPU_TRACING"] = "true"
    env["MICROMEGAS_TELEMETRY_URL"] = args.telemetry_url

    build_cmd = ["cargo", "build", "--bin", "telemetry-generator"]
    run_cmd = [
        "cargo",
        "run",
        "--bin",
        "telemetry-generator",
    ]
    if args.release:
        build_cmd.append("--release")
        run_cmd.append("--release")
    run_cmd += [
        "--",
        "--tasks",
        str(args.tasks),
        "--async-tasks",
        str(args.async_tasks),
        "--threads",
        str(args.threads),
    ]

    print(f"Building telemetry-generator ({'release' if args.release else 'debug'})...")
    subprocess.run(build_cmd, cwd=RUST_DIR, env=env, check=True)

    print(
        f"Running generator -> {args.telemetry_url} "
        f"(tasks={args.tasks}, async_tasks={args.async_tasks}, threads={args.threads}, "
        "cpu_tracing=on)"
    )
    result = subprocess.run(run_cmd, cwd=RUST_DIR, env=env)
    if result.returncode != 0:
        print(f"generator exited with code {result.returncode}", file=sys.stderr)
        sys.exit(result.returncode)
    print("generator completed; thread/async span data flushed to ingestion.")


if __name__ == "__main__":
    main()
