import grpc
from . import time
from . import perfetto
from . import flightsql


def connect():
    "connect to the analytics service using default values"
    channel_cred = grpc.local_channel_credentials()
    return flightsql.client.FlightSQLClient("localhost:50051", channel_cred)
