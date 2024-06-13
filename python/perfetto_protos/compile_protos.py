#!/bin/python3
import subprocess
import pathlib
import os

perfetto_folder = (
    pathlib.Path(__file__).parent.parent.parent.parent.absolute() / "external/perfetto"
)
python_output_folder = (
    pathlib.Path(__file__).parent.parent.absolute()
    / "micromegas/micromegas/thirdparty/perfetto/"
)


def run_command(cmd):
    print("cmd=", cmd)
    subprocess.run(cmd, shell=True, check=True)


if not python_output_folder.exists():
    os.makedirs(str(python_output_folder))

run_command(
    "protoc --dependency_out=dep.txt --proto_path={perfetto} --python_out={python_out} protos/perfetto/trace/trace.proto".format(
        perfetto=perfetto_folder, python_out=python_output_folder
    )
)

for line in open("dep.txt", "r"):
    proto_dep = line.split()[-1].replace(".proto\\", ".proto")
    run_command(
        "protoc --proto_path={perfetto} --python_out={python_out} {proto}".format(
            perfetto=perfetto_folder,
            python_out=python_output_folder,
            proto=proto_dep,
        )
    )
