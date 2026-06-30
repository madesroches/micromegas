#!/usr/bin/env python3

"""
Run a local MinIO container as an S3-compatible object store, point the
micromegas object cache at it, and launch the rest of the services.

The object cache (micromegas-object-cache-srv) only starts when
MICROMEGAS_OBJECT_STORE_URI resolves to a bucket-style origin (s3://, gs://);
a file:// lake (the start_services.py default) can't derive one, so the
cache gets silently skipped. This script gives you a real S3 endpoint to
exercise the cache against without needing AWS credentials.

Usage: python3 start_minio.py [--bucket NAME] [--lake-prefix PATH]
                               [--s3-port PORT] [--console-port PORT]
                               [--no-launch] [-- <start_services.py args>]

Anything after the script's own flags is forwarded to start_services.py,
e.g.: python3 start_minio.py --monolith
"""

import argparse
import os
import sys
import subprocess
import time
from pathlib import Path

import docker
import requests

CONTAINER_NAME = "micromegas-minio"
IMAGE = "minio/minio:latest"
ROOT_USER = "minioadmin"
ROOT_PASSWORD = "minioadmin"
CONTAINER_S3_PORT = 9000
CONTAINER_CONSOLE_PORT = 9101


def ensure_minio_running(s3_port, console_port):
    """Start (or reuse) the MinIO container. Returns the host S3 port actually bound."""
    client = docker.from_env()
    try:
        container = client.containers.get(CONTAINER_NAME)
        if container.status != "running":
            print(f"\U0001f504 Starting existing {CONTAINER_NAME} container...")
            container.start()
            container.reload()
        else:
            print(f"✅ {CONTAINER_NAME} container already running")
        bound_port = container.attrs["NetworkSettings"]["Ports"][f"{CONTAINER_S3_PORT}/tcp"][0][
            "HostPort"
        ]
        return int(bound_port)
    except docker.errors.NotFound:
        print(f"\U0001f195 Creating {CONTAINER_NAME} container...")
        client.containers.run(
            IMAGE,
            name=CONTAINER_NAME,
            command=f'server /data --console-address ":{CONTAINER_CONSOLE_PORT}"',
            environment={
                "MINIO_ROOT_USER": ROOT_USER,
                "MINIO_ROOT_PASSWORD": ROOT_PASSWORD,
            },
            ports={
                f"{CONTAINER_S3_PORT}/tcp": ("127.0.0.1", s3_port),
                f"{CONTAINER_CONSOLE_PORT}/tcp": ("127.0.0.1", console_port),
            },
            detach=True,
        )
        return s3_port


def wait_for_minio(s3_port, max_attempts=30):
    print("⏳ Waiting for MinIO...")
    url = f"http://127.0.0.1:{s3_port}/minio/health/live"
    for i in range(1, max_attempts + 1):
        try:
            if requests.get(url, timeout=1).status_code == 200:
                print("✅ MinIO is ready!")
                return True
        except requests.exceptions.RequestException:
            pass
        if i == max_attempts:
            print("❌ MinIO failed to start")
            return False
        time.sleep(1)
    return False


def ensure_bucket(bucket, s3_port):
    import boto3
    from botocore.exceptions import ClientError

    s3 = boto3.client(
        "s3",
        endpoint_url=f"http://127.0.0.1:{s3_port}",
        aws_access_key_id=ROOT_USER,
        aws_secret_access_key=ROOT_PASSWORD,
        region_name="us-east-1",
    )
    try:
        s3.head_bucket(Bucket=bucket)
        print(f"✅ bucket {bucket} already exists")
    except ClientError:
        print(f"\U0001f195 creating bucket {bucket}")
        s3.create_bucket(Bucket=bucket)


def main():
    parser = argparse.ArgumentParser(
        description="Start a local MinIO-backed object store and launch micromegas services"
    )
    parser.add_argument("--bucket", default="micromegas-test")
    parser.add_argument(
        "--lake-prefix",
        default="lake",
        help="Path prefix within the bucket for the telemetry lake",
    )
    parser.add_argument("--s3-port", type=int, default=9100)
    parser.add_argument("--console-port", type=int, default=9101)
    parser.add_argument(
        "--no-launch",
        action="store_true",
        help="Only start MinIO and create the bucket; don't launch start_services.py",
    )
    args, remaining = parser.parse_known_args()

    bound_s3_port = ensure_minio_running(args.s3_port, args.console_port)
    if bound_s3_port != args.s3_port:
        print(
            f"⚠️  {CONTAINER_NAME} is already bound to host port {bound_s3_port}, "
            f"not the requested {args.s3_port}; using {bound_s3_port}"
        )

    if not wait_for_minio(bound_s3_port):
        sys.exit(1)

    ensure_bucket(args.bucket, bound_s3_port)

    env = os.environ.copy()
    env["MICROMEGAS_OBJECT_STORE_URI"] = f"s3://{args.bucket}/{args.lake_prefix}"
    env["AWS_ENDPOINT"] = f"http://127.0.0.1:{bound_s3_port}"
    env["AWS_ACCESS_KEY_ID"] = ROOT_USER
    env["AWS_SECRET_ACCESS_KEY"] = ROOT_PASSWORD
    env["AWS_REGION"] = "us-east-1"
    env["AWS_ALLOW_HTTP"] = "true"

    print(f"Set MICROMEGAS_OBJECT_STORE_URI={env['MICROMEGAS_OBJECT_STORE_URI']}")
    print(f"Set AWS_ENDPOINT={env['AWS_ENDPOINT']}")

    if args.no_launch:
        print()
        print("MinIO is ready. To use it in this shell:")
        for key in (
            "MICROMEGAS_OBJECT_STORE_URI",
            "AWS_ENDPOINT",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_REGION",
            "AWS_ALLOW_HTTP",
        ):
            print(f"  export {key}={env[key]}")
        return

    script_dir = Path(__file__).parent.absolute()
    subprocess.run(
        [sys.executable, str(script_dir / "start_services.py")] + remaining,
        env=env,
        check=True,
    )


if __name__ == "__main__":
    main()
