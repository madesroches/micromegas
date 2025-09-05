#!/usr/bin/python3
import argparse
import subprocess
import os


def main():
    parser = argparse.ArgumentParser(
        prog="dump",
        description="runs pg_dump on the local telemetry database",
    )
    parser.add_argument("file", help="path to the file to write")
    args = parser.parse_args()

    username = os.environ.get("MICROMEGAS_DB_USERNAME")
    port = os.environ.get("MICROMEGAS_DB_PORT")
    password = os.environ.get("MICROMEGAS_DB_PASSWD")
    
    env = os.environ.copy()
    if password:
        env["PGPASSWORD"] = password
    
    subprocess.run(
        "pg_dump -h localhost -p {port} -U {username} --file {file} --format=c".format(
            username=username, port=port, file=args.file
        ),
        shell=True,
        check=True,
        env=env
    )


if __name__ == "__main__":
    main()
