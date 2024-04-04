import os
import pathlib
import subprocess


def run_command(cmd):
    print("cmd=", cmd)
    subprocess.run(cmd, shell=True, check=True)


def make_link(link, target):
    if os.path.exists(link):
        print(link, "exists")
        return
    run_command(
        "mklink /J {link} {target}".format(
            link=link,
            target=target,
        )
    )


unreal_root_dir = os.environ.get("MICROMEGAS_UNREAL_ROOT_DIR")
core_dir = pathlib.Path(unreal_root_dir) / "Engine" / "Source" / "Runtime" / "Core"
micromegas_unreal_root = pathlib.Path(__file__).parent.parent.absolute() / "unreal"

make_link(
    core_dir / "Public" / "MicromegasTracing",
    micromegas_unreal_root / "MicromegasTracing" / "Public" / "MicromegasTracing",
)

make_link(
    core_dir / "Private" / "MicromegasTracing",
    micromegas_unreal_root / "MicromegasTracing" / "Private",
)
