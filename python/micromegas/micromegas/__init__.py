import grpc
from . import time
from . import perfetto
from . import flightsql


def connect():
    "connect to the analytics service using default values"
    return flightsql.client.FlightSQLClient("grpc://localhost:50051")
