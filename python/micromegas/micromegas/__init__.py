import os
from . import time
from . import request
from . import client
from . import perfetto

def connect():
    "connect to the analytics service using default values"
    BASE_URL = "http://localhost:8082/"
    return client.Client(BASE_URL)
