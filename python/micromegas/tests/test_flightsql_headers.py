"""Hermetic unit test for micromegas.flightsql.client.make_call_headers.

make_call_headers is the function on the live query path that actually
formats `begin`/`end` values for the FlightSQL call headers (see
client.py's query()/query_stream()/query_arrow()); it has no I/O, so it
can be tested directly without a service or mocking.
"""

from micromegas.flightsql.client import make_call_headers


def test_make_call_headers_z_suffix_begin():
    headers = make_call_headers("2024-01-01T00:00:00Z", None)
    assert (b"query_range_begin", b"2024-01-01T00:00:00+00:00") in headers
