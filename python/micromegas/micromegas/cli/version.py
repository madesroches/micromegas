import importlib.metadata
import platform
import sys


def package_version():
    """Return the installed micromegas package version, or 'unknown' if not installed."""
    try:
        return importlib.metadata.version("micromegas")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


def add_version_argument(parser):
    """Add a --version flag reporting the micromegas package version and interpreter."""
    version_string = (
        f"%(prog)s {package_version()} "
        f"(Python {platform.python_version()} at {sys.executable})"
    )
    parser.add_argument(
        "--version",
        action="version",
        version=version_string,
    )
