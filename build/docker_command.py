import subprocess
import pathlib

docker_root = pathlib.Path(__file__).parent.parent.absolute() / "docker"

def run_docker_command(cmd):
    print("cmd=", cmd, "cwd=", docker_root)
    subprocess.run(cmd, shell=True, cwd=docker_root, check=True)
