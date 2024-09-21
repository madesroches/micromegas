import subprocess
import pathlib

rust_root = pathlib.Path(__file__).parent.parent.absolute() / "rust"

def run_command(cmd):
    print("cmd=", cmd, "cwd=", rust_root)
    subprocess.run(cmd, shell=True, cwd=rust_root, check=True)
