import grpc
from . import admin
from . import auth
from . import flightsql
from . import oidc_connection
from . import perfetto
from . import time


def connect(uri=None, preserve_dictionary=False):
    """Connect to the analytics service.

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
