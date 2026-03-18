#!/usr/bin/env python3
"""
Management script for the self-hosted GitHub Actions runner container.
See tasks/container_based_dev_worker_plan.md for design details.

Usage:
    # Start the worker (runs until stopped with Ctrl+C)
    python3 build/dev_worker.py

    # With resource limits
    python3 build/dev_worker.py --cpus 8 --memory 16g

    # Clear the build cache and exit
    python3 build/dev_worker.py --clear-cache

    # Rotate cache: clear, restart worker, trigger warming build
    python3 build/dev_worker.py --rotate-cache

    # Build the container image only
    python3 build/dev_worker.py --build-image

    # Start with nightly cache rotation at 03:00 local time
    python3 build/dev_worker.py --rotate-at 3

PAT setup (choose one):
    export MICROMEGAS_RUNNER_PAT=ghp_xxx
    # or
    echo "ghp_xxx" > ~/.config/micromegas/runner-pat && chmod 600 ~/.config/micromegas/runner-pat
"""

import argparse
import datetime
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

REPO = "madesroches/micromegas"
IMAGE_NAME = "micromegas-github-runner"
VOLUME_NAME = "micromegas-build-cache"
CONTAINER_NAME = "micromegas-runner"
PAT_FILE = os.path.expanduser("~/.config/micromegas/runner-pat")
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def get_pat():
    """Read PAT from env var or file."""
    pat = os.environ.get("MICROMEGAS_RUNNER_PAT")
    if pat:
        return pat.strip()
    if os.path.exists(PAT_FILE):
        mode = oct(os.stat(PAT_FILE).st_mode)[-3:]
        if mode != "600":
            print(f"WARNING: {PAT_FILE} has permissions {mode}, expected 600")
        with open(PAT_FILE) as f:
            return f.read().strip()
    print(
        f"Error: No PAT found.\n"
        f"  Option 1: export MICROMEGAS_RUNNER_PAT=ghp_xxx\n"
        f"  Option 2: echo 'ghp_xxx' > {PAT_FILE} && chmod 600 {PAT_FILE}"
    )
    sys.exit(1)


def github_api(path, pat, method="GET", data=None):
    """Make a GitHub API request."""
    url = f"https://api.github.com/{path}"
    headers = {
        "Authorization": f"Bearer {pat}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    body = json.dumps(data).encode() if data else None
    req = urllib.request.Request(url, data=body, headers=headers, method=method)
    with urllib.request.urlopen(req) as resp:
        return json.loads(resp.read())


def get_registration_token(pat):
    """Get a short-lived runner registration token from the GitHub API."""
    result = github_api(
        f"repos/{REPO}/actions/runners/registration-token", pat, method="POST"
    )
    return result["token"]


def get_arch():
    """Return the runner architecture label."""
    machine = platform.machine()
    if machine in ("x86_64", "AMD64"):
        return "x64"
    if machine in ("aarch64", "arm64"):
        return "arm64"
    return machine


def build_image():
    """Build the runner container image from the repo root."""
    print(f"Building {IMAGE_NAME} image...")
    subprocess.run(
        [
            "docker",
            "build",
            "-f",
            "docker/github-runner.Dockerfile",
            "-t",
            IMAGE_NAME,
            ".",
        ],
        cwd=REPO_ROOT,
        check=True,
    )
    print(f"Image {IMAGE_NAME} built successfully.")


def start_container(pat, cpus=None, memory=None):
    """Start a persistent runner container. Returns (Popen, token_path)."""
    token = get_registration_token(pat)
    arch = get_arch()

    # Write token to a temp file (never pass via env var or CLI)
    fd, token_path = tempfile.mkstemp(prefix="runner-token-")
    os.write(fd, token.encode())
    os.close(fd)
    os.chmod(token_path, 0o600)

    cmd = [
        "docker",
        "run",
        "--name",
        CONTAINER_NAME,
        "--rm",
        "-e",
        f"REPO={REPO}",
        "-e",
        f"RUNNER_NAME={CONTAINER_NAME}",
        "-e",
        f"ARCH={arch}",
        "-v",
        f"{VOLUME_NAME}:/cache",
        "--mount",
        f"type=bind,source={token_path},target=/run/secrets/registration-token,readonly",
    ]
    if cpus:
        cmd.extend(["--cpus", str(cpus)])
    if memory:
        cmd.extend(["--memory", str(memory)])
    cmd.append(IMAGE_NAME)

    try:
        proc = subprocess.Popen(cmd)
    except Exception:
        os.unlink(token_path)
        raise
    return proc, token_path


def stop_container():
    """Stop the running container if any."""
    subprocess.run(["docker", "stop", CONTAINER_NAME], capture_output=True)


def delete_volume():
    """Delete the build cache volume."""
    result = subprocess.run(
        ["docker", "volume", "rm", VOLUME_NAME], capture_output=True
    )
    if result.returncode == 0:
        print(f"Cache volume {VOLUME_NAME} removed.")
    else:
        print(f"Volume {VOLUME_NAME} not found or already removed.")


def clear_cache():
    """Stop runner and delete the build cache volume."""
    stop_container()
    # Give container time to exit
    time.sleep(2)
    delete_volume()


def is_runner_online(pat):
    """Check if a dev-worker runner is online."""
    try:
        result = github_api(f"repos/{REPO}/actions/runners", pat)
        for runner in result.get("runners", []):
            labels = [label["name"] for label in runner.get("labels", [])]
            if "dev-worker" in labels and runner.get("status") == "online":
                return True
    except Exception:
        pass
    return False


def wait_for_runner_online(pat, timeout=120):
    """Poll until a dev-worker runner is online or timeout."""
    print("Waiting for runner to register as online...")
    start = time.time()
    while time.time() - start < timeout:
        if is_runner_online(pat):
            print("Runner is online.")
            return True
        time.sleep(5)
    print(f"Runner did not come online within {timeout}s.")
    return False


def trigger_warming_build(pat):
    """Trigger rust.yml workflow_dispatch on main to warm the cache."""
    try:
        github_api(
            f"repos/{REPO}/actions/workflows/rust.yml/dispatches",
            pat,
            method="POST",
            data={"ref": "main"},
        )
        print("Triggered warming build on main.")
    except Exception as e:
        print(f"Failed to trigger warming build: {e}")


def seconds_until(hour, minute=0):
    """Return seconds from now until the next occurrence of hour:minute local time."""
    now = datetime.datetime.now()
    target = now.replace(hour=hour, minute=minute, second=0, microsecond=0)
    if target <= now:
        target += datetime.timedelta(days=1)
    return (target - now).total_seconds()


def nightly_rotation_thread(rotation_event, rotate_hour):
    """Sleep until rotate_hour:00 each night, then signal the main loop to rotate."""
    while True:
        wait = seconds_until(rotate_hour)
        print(f"Nightly cache rotation scheduled in {wait / 3600:.1f}h (at {rotate_hour:02d}:00)")
        time.sleep(wait)
        print("Nightly cache rotation triggered.")
        rotation_event.set()


def run_worker_loop(pat, cpus=None, memory=None, trigger_warming=False, rotate_hour=None):
    """Main loop: start persistent container, restart on exit or rotation."""
    running = True
    rotation_event = threading.Event()

    if rotate_hour is not None:
        t = threading.Thread(
            target=nightly_rotation_thread,
            args=(rotation_event, rotate_hour),
            daemon=True,
        )
        t.start()

    def handle_signal(sig, _frame):
        nonlocal running
        print("\nShutting down...")
        running = False
        stop_container()

    signal.signal(signal.SIGINT, handle_signal)
    signal.signal(signal.SIGTERM, handle_signal)

    print(f"Starting dev worker for {REPO}")
    print("Press Ctrl+C to stop\n")

    while running:
        # Check if nightly rotation was requested
        if rotation_event.is_set():
            rotation_event.clear()
            print("Performing nightly cache rotation...")
            stop_container()
            time.sleep(2)
            delete_volume()
            trigger_warming = True

        try:
            proc, token_path = start_container(pat, cpus=cpus, memory=memory)
        except Exception as e:
            if running:
                print(f"Failed to start container: {e}")
                print("Retrying in 10 seconds...")
                time.sleep(10)
            continue

        try:
            # After rotate-cache or nightly rotation, trigger a warming build
            if trigger_warming:
                if wait_for_runner_online(pat):
                    trigger_warming_build(pat)
                trigger_warming = False

            proc.wait()
        finally:
            # Clean up the host-side token file
            try:
                os.unlink(token_path)
            except OSError:
                pass

        if running:
            print("Container exited unexpectedly, restarting runner...\n")


def main():
    parser = argparse.ArgumentParser(
        description="Manage the self-hosted GitHub Actions runner container",
        epilog="Full documentation: https://micromegas.info/development/build/#self-hosted-ci-runner",
    )
    parser.add_argument("--cpus", help="CPU limit for container (e.g., 8)")
    parser.add_argument("--memory", help="Memory limit for container (e.g., 16g)")
    parser.add_argument(
        "--clear-cache", action="store_true", help="Clear build cache and exit"
    )
    parser.add_argument(
        "--rotate-cache",
        action="store_true",
        help="Clear cache, restart worker, trigger warming build",
    )
    parser.add_argument(
        "--build-image", action="store_true", help="Build the container image and exit"
    )
    parser.add_argument(
        "--rotate-at",
        type=int,
        metavar="HOUR",
        help="Nightly cache rotation hour in local time (0-23, e.g., 3 for 03:00)",
    )
    args = parser.parse_args()

    if args.build_image:
        build_image()
        return

    pat = get_pat()

    if args.clear_cache:
        clear_cache()
        return

    trigger_warming = False
    if args.rotate_cache:
        print("Rotating cache...")
        clear_cache()
        trigger_warming = True

    build_image()
    run_worker_loop(
        pat,
        cpus=args.cpus,
        memory=args.memory,
        trigger_warming=trigger_warming,
        rotate_hour=args.rotate_at,
    )


if __name__ == "__main__":
    main()
