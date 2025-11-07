import subprocess
import pathlib
import shutil

rust_root = pathlib.Path(__file__).parent.parent.absolute() / "rust"

def show_disk_space():
    """Show disk space usage"""
    try:
        total, used, free = shutil.disk_usage("/")
        gb = 1024 ** 3
        print(f"💾 Disk: {used/gb:.1f}GB used / {total/gb:.1f}GB total ({free/gb:.1f}GB free, {100*used/total:.0f}% used)")
    except Exception as e:
        print(f"⚠️  Could not get disk space: {e}")

def run_command(cmd):
    print("cmd=", cmd, "cwd=", rust_root)
    subprocess.run(cmd, shell=True, cwd=rust_root, check=True)
    show_disk_space()
