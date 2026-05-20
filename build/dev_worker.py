#!/usr/bin/env python3
"""
Management script for the self-hosted GitHub Actions runner container.
See tasks/container_based_dev_worker_plan.md for design details.

Usage:
    # Start the worker (runs until stopped with Ctrl+C)
    python3 build/dev_worker.py

    # With resource limits
    python3 build/dev_worker.py --cpus 8 --memory 16g

    # Build the container image only
    python3 build/dev_worker.py --build-image

PAT setup (choose one):
    export MICROMEGAS_RUNNER_PAT=ghp_xxx
    # or
    echo "ghp_xxx" > ~/.config/micromegas/runner-pat && chmod 600 ~/.config/micromegas/runner-pat
"""

import argparse
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
import uuid

REPO = "madesroches/micromegas"
IMAGE_NAME = "micromegas-github-runner"
CONTAINER_NAME_PREFIX = "micromegas-runner"
# Label applied to each runner container so we can find them by query without
# needing to know the unique per-run container name.
CONTAINER_LABEL = "com.micromegas.runner=dev-worker"
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
        body = resp.read()
        return json.loads(body) if body else None


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


def get_latest_runner_version():
    """Query the actions/runner repo for the latest release tag (e.g. '2.334.0').

    Returns None on failure so the Dockerfile's default RUNNER_VERSION is used.
    """
    try:
        req = urllib.request.Request(
            "https://api.github.com/repos/actions/runner/releases/latest",
            headers={"Accept": "application/vnd.github+json"},
        )
        with urllib.request.urlopen(req, timeout=10) as resp:
            result = json.loads(resp.read())
    except Exception as e:
        print(f"Failed to fetch latest runner version: {e}")
        return None
    tag = result.get("tag_name", "") if result else ""
    return tag.lstrip("v") or None


def build_image():
    """Build the runner container image from the repo root."""
    print(f"Building {IMAGE_NAME} image...")
    cmd = ["docker", "build", "-f", "docker/github-runner.Dockerfile", "-t", IMAGE_NAME]
    runner_version = get_latest_runner_version()
    if runner_version:
        print(f"Pinning RUNNER_VERSION={runner_version}")
        cmd.extend(["--build-arg", f"RUNNER_VERSION={runner_version}"])
    cmd.append(".")
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)
    print(f"Image {IMAGE_NAME} built successfully.")


def start_container(pat, cpus=None, memory=None):
    """Start an ephemeral runner container. Returns (Popen, token_path).

    Each run gets a unique container name so successive containers cannot
    conflict on Docker's name registry while the previous --rm cleanup is
    still draining. The GitHub runner registers under the same unique name.
    """
    token = get_registration_token(pat)
    arch = get_arch()
    name = f"{CONTAINER_NAME_PREFIX}-{uuid.uuid4().hex[:8]}"

    # Write token to a temp file (never pass via env var or CLI)
    fd, token_path = tempfile.mkstemp(prefix="runner-token-")
    os.write(fd, token.encode())
    os.close(fd)
    os.chmod(token_path, 0o600)

    cmd = [
        "docker",
        "run",
        "--name",
        name,
        "--label",
        CONTAINER_LABEL,
        "--rm",
        "-e",
        f"REPO={REPO}",
        "-e",
        f"RUNNER_NAME={name}",
        "-e",
        f"ARCH={arch}",
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
    """Stop any currently running runner containers (located via label)."""
    result = subprocess.run(
        ["docker", "ps", "-q", "--filter", f"label={CONTAINER_LABEL}"],
        capture_output=True,
        text=True,
    )
    ids = result.stdout.split()
    if ids:
        subprocess.run(["docker", "stop", *ids], capture_output=True)


def cleanup_offline_runners(pat):
    """Remove any offline dev-worker runners from the repository."""
    try:
        result = github_api(f"repos/{REPO}/actions/runners", pat)
        for runner in result.get("runners", []):
            labels = [label["name"] for label in runner.get("labels", [])]
            if "dev-worker" in labels and runner.get("status") == "offline":
                runner_id = runner["id"]
                runner_name = runner.get("name", runner_id)
                try:
                    github_api(
                        f"repos/{REPO}/actions/runners/{runner_id}",
                        pat,
                        method="DELETE",
                    )
                    print(f"Removed offline runner: {runner_name}")
                except Exception as e:
                    print(f"Failed to remove runner {runner_name}: {e}")
    except Exception as e:
        print(f"Failed to list runners for cleanup: {e}")


def run_worker_loop(pat, cpus=None, memory=None):
    """Main loop: start ephemeral containers, replace each one after it exits."""
    running = True

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
        try:
            proc, token_path = start_container(pat, cpus=cpus, memory=memory)
        except Exception as e:
            if running:
                print(f"Failed to start container: {e}")
                print("Retrying in 10 seconds...")
                time.sleep(10)
            continue

        try:
            proc.wait()
        finally:
            try:
                os.unlink(token_path)
            except OSError:
                pass

        if running:
            print("Runner finished its job, starting next ephemeral runner...\n")


def main():
    parser = argparse.ArgumentParser(
        description="Manage the self-hosted GitHub Actions runner container",
        epilog="Full documentation: https://micromegas.info/development/build/#self-hosted-ci-runner",
    )
    parser.add_argument("--cpus", help="CPU limit for container (e.g., 8)")
    parser.add_argument("--memory", help="Memory limit for container (e.g., 16g)")
    parser.add_argument(
        "--build-image", action="store_true", help="Build the container image and exit"
    )
    parser.add_argument(
        "--cleanup",
        action="store_true",
        help="Remove offline dev-worker runners from GitHub and exit",
    )
    args = parser.parse_args()

    if args.build_image:
        build_image()
        return

    pat = get_pat()

    if args.cleanup:
        cleanup_offline_runners(pat)
        return

    cleanup_offline_runners(pat)
    build_image()
    run_worker_loop(pat, cpus=args.cpus, memory=args.memory)


if __name__ == "__main__":
    main()
