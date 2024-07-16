#!/usr/bin/python3
import argparse
import subprocess
import os


def main():
    parser = argparse.ArgumentParser(
        prog="restore",
        description="runs pg_restore on the local telemetry database",
    )
    parser.add_argument("file", help="path to the file to write")
    args = parser.parse_args()

    username = os.environ.get("MICROMEGAS_DB_USERNAME")
    subprocess.run(
        "pg_restore -h localhost -p 5432 -U {username} -d telemetry {file}".format(
            username=username, file=args.file
        ),
        shell=True,
        check=True,
    )


if __name__ == "__main__":
    main()
