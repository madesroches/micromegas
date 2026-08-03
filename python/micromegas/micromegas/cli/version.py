import argparse
import importlib.metadata
import platform
import sys


def package_version():
    """Return the installed micromegas package version, or 'unknown' if not installed."""
    try:
        return importlib.metadata.version("micromegas")
    except importlib.metadata.PackageNotFoundError:
        return "unknown"


class _VerbatimVersionAction(argparse.Action):
    """Like argparse's built-in "version" action, but prints the string as-is
    instead of routing it through HelpFormatter, which reflows (and can break
    mid-token) long paths such as the interpreter path."""

    def __init__(
        self,
        option_strings,
        version=None,
        dest=argparse.SUPPRESS,
        default=argparse.SUPPRESS,
        help="show program's version number and exit",
    ):
        super().__init__(
            option_strings=option_strings,
            dest=dest,
            default=default,
            nargs=0,
            help=help,
        )
        self.version = version

    def __call__(self, parser, namespace, values, option_string=None):
        version = self.version
        if version is None:
            version = parser.version
        print(version)
        parser.exit(0)


def add_version_argument(parser):
    """Add a --version flag reporting the micromegas package version and interpreter."""
    version_string = (
        f"{parser.prog} {package_version()} "
        f"(Python {platform.python_version()} at {sys.executable})"
    )
    parser.add_argument(
        "--version",
        action=_VerbatimVersionAction,
        version=version_string,
    )
