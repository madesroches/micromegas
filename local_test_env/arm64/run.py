#!/usr/bin/python3
import subprocess
import pathlib

root = pathlib.Path(__file__).parent.parent.parent.absolute()

subprocess.run(
    "docker run -v {root}:/micromegas -it mmarm64 bash".format(root=root),
    shell=True,
    check=True,
)
