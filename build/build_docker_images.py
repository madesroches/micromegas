#!/usr/bin/env python3
"""
Build and push Docker images for micromegas services.

Usage:
    python build_docker_images.py                    # Build all services
    python build_docker_images.py ingestion flight   # Build specific services
    python build_docker_images.py --push             # Build and push all
    python build_docker_images.py --push ingestion   # Build and push specific service
    python build_docker_images.py --list             # List available services
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
    "admin": ("admin.Dockerfile", "Telemetry admin CLI"),
    "http-gateway": ("http-gateway.Dockerfile", "HTTP gateway server"),
    "analytics-web": ("analytics-web.Dockerfile", "Analytics web app"),
    "all": ("all-in-one.Dockerfile", "All services in one image (dev/test)"),
}


def get_version() -> str:
    """Read version from root Cargo.toml"""
    cargo_toml = REPO_ROOT / "rust" / "Cargo.toml"
    content = cargo_toml.read_text()

    # Find version in [workspace.package] section
    match = re.search(r'\[workspace\.package\].*?version\s*=\s*"([^"]+)"', content, re.DOTALL)
    if match:
        return match.group(1)

    # Fallback: find first version
    match = re.search(r'version\s*=\s*"([^"]+)"', content)
    if match:
        return match.group(1)

    raise ValueError("Could not find version in Cargo.toml")


def run_command(cmd: list[str], cwd: Path = REPO_ROOT) -> bool:
    """Run a command and return success status"""
    print(f">>> {' '.join(cmd)}")
    result = subprocess.run(cmd, cwd=cwd)
    return result.returncode == 0


def build_image(service: str, version: str, push: bool = False) -> dict:
    """Build a Docker image for a service.

    Returns a dict with build results:
        - 'service': service name
        - 'image': full image name
        - 'version': version tag
        - 'built': True if build succeeded
        - 'pushed': True if push succeeded (only if push=True)
    """
    result = {
        'service': service,
        'image': None,
        'version': version,
        'built': False,
        'pushed': False,
    }

    if service not in SERVICES:
        print(f"Unknown service: {service}")
        return result

    dockerfile, description = SERVICES[service]
    image_name = f"{DOCKERHUB_USER}/{DOCKERHUB_REPO}-{service}"
    result['image'] = image_name

    print(f"\n{'='*60}")
    print(f"Building {service}: {description}")
    print(f"Image: {image_name}:{version}")
    print(f"{'='*60}\n")

    # Build with version tag and latest tag
    cmd = [
        "docker", "build",
        "-f", str(DOCKER_DIR / dockerfile),
        "-t", f"{image_name}:{version}",
        "-t", f"{image_name}:latest",
        "."
    ]

    if not run_command(cmd):
        print(f"Failed to build {service}")
        return result

    result['built'] = True

    if push:
        print(f"\nPushing {image_name}...")
        if not run_command(["docker", "push", f"{image_name}:{version}"]):
            return result
        if not run_command(["docker", "push", f"{image_name}:latest"]):
            return result
        result['pushed'] = True

    return result


def main():
    parser = argparse.ArgumentParser(
        description="Build Docker images for micromegas services",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__
    )
    parser.add_argument(
        "services",
        nargs="*",
        help="Services to build (default: all)"
    )
    parser.add_argument(
        "--push",
        action="store_true",
        help="Push images to DockerHub after building"
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="List available services"
    )
    parser.add_argument(
        "--version",
        help="Override version (default: from Cargo.toml)"
    )

    args = parser.parse_args()

    if args.list:
        print("Available services:")
        for name, (dockerfile, desc) in SERVICES.items():
            print(f"  {name:15} - {desc}")
        return 0

    version = args.version or get_version()
    print(f"Version: {version}")

    # Default: build all individual services but not 'all' (redundant)
    services = args.services or [s for s in SERVICES.keys() if s != "all"]

    # Validate services
    for service in services:
        if service not in SERVICES:
            print(f"Unknown service: {service}")
            print(f"Available: {', '.join(SERVICES.keys())}")
            return 1

    # Build each service
    results = []
    for service in services:
        result = build_image(service, version, args.push)
        results.append(result)

    # Print summary
    print(f"\n{'='*60}")
    print("BUILD SUMMARY")
    print(f"{'='*60}")
    print(f"Version: {version}")
    print()

    built = [r for r in results if r['built']]
    failed = [r for r in results if not r['built']]
    pushed = [r for r in results if r['pushed']]

    if built:
        print("Built images:")
        for r in built:
            status = " (pushed)" if r['pushed'] else ""
            print(f"  {r['image']}:{r['version']}{status}")
            print(f"  {r['image']}:latest{status}")

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
