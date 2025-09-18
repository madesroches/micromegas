import grpc
from . import time
from . import perfetto
from . import flightsql
from . import admin


def connect(preserve_dictionary=False):
    """Connect to the analytics service using default values.

    Args:
        preserve_dictionary (bool, optional): When True, preserve dictionary encoding in
            Arrow arrays for memory efficiency. Useful when using dictionary-encoded UDFs.
            Defaults to False for backward compatibility.
    """
    return flightsql.client.FlightSQLClient(
        "grpc://localhost:50051", preserve_dictionary=preserve_dictionary
    )
