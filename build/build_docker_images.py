#!/usr/bin/env python3
"""
Build and push Docker images for micromegas services.

Every image is tagged `<version>`, `latest`, and `<sha12>` (the first 12 characters of the
HEAD commit sha, with `-dirty` appended if the worktree has uncommitted changes), each with
`-arm64` appended under `--arm64`. Every image also carries the OCI label
`org.opencontainers.image.revision=<full-sha>[-dirty]`.

Usage:
    python build_docker_images.py                         # Build all services (amd64)
    python build_docker_images.py ingestion flight-sql    # Build specific services
    python build_docker_images.py --push                  # Build and push amd64 images to Docker Hub
    python build_docker_images.py --push ingestion        # Build and push specific service
    python build_docker_images.py --arm64                 # Build arm64 locally (cross-compiled, no push)
    python build_docker_images.py --arm64 --push          # Build and push arm64 images to Docker Hub
    python build_docker_images.py --all-arches            # Build both amd64 and arm64 locally
    python build_docker_images.py --all-arches --push     # Build and push both amd64 and arm64 (release)
    python build_docker_images.py --list                  # List available services
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

# Repository root
REPO_ROOT = Path(__file__).parent.parent.absolute()
DOCKER_DIR = REPO_ROOT / "docker"

# DockerHub configuration
DOCKERHUB_USER = "marcantoinedesroches"
DOCKERHUB_REPO = "micromegas"

# Service definitions: name -> (dockerfile, description)
SERVICES = {
    "ingestion": ("ingestion.Dockerfile", "Telemetry ingestion server"),
    "flight-sql": ("flight-sql.Dockerfile", "FlightSQL analytics server"),
    "maintenance": ("maintenance.Dockerfile", "Maintenance daemon"),
    "object-cache": ("object-cache.Dockerfile", "Shared object range cache service"),
    "http-gateway": ("http-gateway.Dockerfile", "HTTP gateway server"),
    "analytics-web": ("analytics-web.Dockerfile", "Analytics web app"),
    "redis-exporter": ("redis-exporter.Dockerfile", "Redis metrics exporter"),
    "all": ("all-in-one.Dockerfile", "All services in one image (dev/test)"),
    "monolith": ("monolith.Dockerfile", "Single-process monolith (all roles)"),
}


def get_version() -> str:
    """Read version from root Cargo.toml"""
    cargo_toml = REPO_ROOT / "rust" / "Cargo.toml"
    content = cargo_toml.read_text()

    # Find version in [workspace.package] section
    match = re.search(
        r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"', content, re.DOTALL
    )
    if match:
        return match.group(1)

    # Fallback: find first version
    match = re.search(r'version\s*=\s*"([^"]+)"', content)
    if match:
        return match.group(1)

    raise ValueError("Could not find version in Cargo.toml")


def get_revision(cwd: Path = REPO_ROOT) -> str:
    """`<full-sha>` of HEAD, with `-dirty` appended when the worktree has changes.

    Untracked files count as changes: unless `.dockerignore` excludes them, they're part of
    the build context.
    """
    sha = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.strip()
    status = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    if status.strip():
        sha += "-dirty"
    return sha


def image_tags(version: str, revision: str, arm64: bool) -> list[str]:
    """Tags to apply to an image: `[version, "latest", revision_tag]`.

    `-arm64` is appended to each when `arm64` is set. The revision tag is the first 12
    characters of `revision`, plus `-dirty` when `revision` ends with `-dirty`.
    """
    if revision.endswith("-dirty"):
        sha_tag = f"{revision[:-len('-dirty')][:12]}-dirty"
    else:
        sha_tag = revision[:12]

    tags = [version, "latest", sha_tag]
    if arm64:
        tags = [f"{tag}-arm64" for tag in tags]
    return tags


def build_command(
    dockerfile: str,
    image_name: str,
    tags: list[str],
    revision: str,
    arm64: bool,
    push: bool,
) -> list[str]:
    """Assemble the `docker build`/`docker buildx build` command for one image."""
    if arm64:
        cmd = [
            "docker",
            "buildx",
            "build",
            "--platform",
            "linux/arm64",
            "--push" if push else "--load",
        ]
    else:
        cmd = ["docker", "build"]

    cmd += ["-f", str(DOCKER_DIR / dockerfile)]
    for tag in tags:
        cmd += ["-t", f"{image_name}:{tag}"]
    cmd += ["--label", f"org.opencontainers.image.revision={revision}"]
    cmd += ["."]
    return cmd


def run_command(cmd: list[str], cwd: Path = REPO_ROOT) -> bool:
    """Run a command and return success status"""
    print(f">>> {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=cwd)
    return result.returncode == 0


def build_image(
    service: str,
    version: str,
    revision: str,
    push: bool = False,
    arm64: bool = False,
) -> dict:
    """Build a Docker image for a service.

    Returns a dict with build results:
        - 'service': service name
        - 'image': full image name
        - 'tags': list of tags applied
        - 'built': True if build succeeded
        - 'pushed': True if push succeeded (only if push=True)
    """
    result = {
        "service": service,
        "image": None,
        "tags": [],
        "built": False,
        "pushed": False,
    }

    if service not in SERVICES:
        print(f"Unknown service: {service}")
        return result

    dockerfile, description = SERVICES[service]
    image_name = f"{DOCKERHUB_USER}/{DOCKERHUB_REPO}-{service}"
    result["image"] = image_name

    tags = image_tags(version, revision, arm64)
    result["tags"] = tags

    print(f"\n{'='*60}")
    print(f"Building {service}: {description}")
    print(f"Image: {image_name}:{tags[0]}")
    print(f"{'='*60}\n")

    cmd = build_command(dockerfile, image_name, tags, revision, arm64, push)
    arch_suffix = " (arm64)" if arm64 else ""

    if not run_command(cmd):
        action = "build/push" if arm64 and push else "build"
        print(f"Failed to {action} {service}{arch_suffix}")
        return result

    result["built"] = True

    if arm64:
        # buildx already pushed (or loaded) the image as part of the build above.
        result["pushed"] = push
    elif push:
        print(f"\nPushing {image_name}...")
        for tag in tags:
            if not run_command(["docker", "push", f"{image_name}:{tag}"]):
                return result
        result["pushed"] = True

    return result


def main():
    parser = argparse.ArgumentParser(
        description="Build Docker images for micromegas services",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("services", nargs="*", help="Services to build (default: all)")
    parser.add_argument(
        "--push", action="store_true", help="Push images to DockerHub after building"
    )
    parser.add_argument(
        "--arm64",
        action="store_true",
        help="Build linux/arm64 images via cross-compilation (uses docker buildx; add --push to publish to Docker Hub)",
    )
    parser.add_argument(
        "--all-arches",
        action="store_true",
        help="Build both amd64 and arm64 images in one run (add --push to publish; rejects --arm64 as redundant)",
    )
    parser.add_argument("--list", action="store_true", help="List available services")
    parser.add_argument("--version", help="Override version (default: from Cargo.toml)")

    args = parser.parse_args()

    if args.all_arches and args.arm64:
        print("error: --all-arches already includes arm64; --arm64 is redundant")
        return 1

    if args.list:
        print("Available services:")
        for name, (dockerfile, desc) in SERVICES.items():
            print(f"  {name:15} - {desc}")
        return 0

    version = args.version or get_version()
    print(f"Version: {version}")

    try:
        revision = get_revision()
    except (subprocess.CalledProcessError, FileNotFoundError) as e:
        print(f"error: could not determine git revision: {e}")
        return 1
    print(f"Revision: {revision}")

    # Default: build all individual services but not 'all' (dev/test only, not published)
    services = args.services or [s for s in SERVICES.keys() if s != "all"]

    # Validate services
    for service in services:
        if service not in SERVICES:
            print(f"Unknown service: {service}")
            print(f"Available: {', '.join(SERVICES.keys())}")
            return 1

    # --all-arches builds both arches; otherwise build the single selected arch.
    # --push controls publishing independently of arch selection.
    arches = [False, True] if args.all_arches else [args.arm64]
    results = []
    for service in services:
        for arm64 in arches:
            results.append(build_image(service, version, revision, args.push, arm64))

    # Print summary
    print(f"\n{'='*60}")
    print("BUILD SUMMARY")
    print(f"{'='*60}")
    print(f"Version: {version}")
    print(f"Revision: {revision}")
    print()

    built = [r for r in results if r["built"]]
    failed = [r for r in results if not r["built"]]
    pushed = [r for r in results if r["pushed"]]

    if built:
        print("Built images:")
        for r in built:
            status = " (pushed)" if r["pushed"] else ""
            for tag in r["tags"]:
                print(f"  {r['image']}:{tag}{status}")

    if failed:
        print("\nFailed:")
        for r in failed:
            print(f"  {r['service']}")

    print()
    print(f"Total: {len(built)}/{len(results)} built", end="")
    if args.push:
        print(f", {len(pushed)}/{len(results)} pushed", end="")
    print()

    if failed:
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
