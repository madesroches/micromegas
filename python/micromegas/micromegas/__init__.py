import grpc
from . import admin
from . import auth
from . import flightsql
from . import oidc_connection
from . import perfetto
from . import time
from .cli.config import ProfileError
from .cli.version import package_version
from .connection import connect_with_profile

__version__ = package_version()


def connect(uri=None, preserve_dictionary=False):
    """Connect to the analytics service at a plain URI. No config, no auth.

    Takes a single URI and nothing else: it never reads
    `~/.micromegas/config.json` and never authenticates the connection it
    returns. For a named profile (env vars, `~/.micromegas/config.json`,
    OIDC or a static API key), use `connect_with_profile()` instead.

    Args:
        uri (str, optional): FlightSQL server URI. Defaults to "grpc://localhost:50051".
        preserve_dictionary (bool, optional): When True, preserve dictionary encoding in
            Arrow arrays for memory efficiency. Useful when using dictionary-encoded UDFs.
            Defaults to False for backward compatibility.
    """
    if uri is None:
        uri = "grpc://localhost:50051"
    return flightsql.client.FlightSQLClient(
        uri, preserve_dictionary=preserve_dictionary
    )
