from .test_utils import *


def test_streams_query():
    streams = client.query_streams(begin, end, limit)
    print(streams.info())

    sql = "select * from streams LIMIT 1024;"
    streams = client.query_view("streams", "global", begin, end, sql)
    print(streams)
