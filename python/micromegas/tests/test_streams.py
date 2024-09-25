from .test_utils import *


def test_streams_query():
    sql = "select * from streams LIMIT 10;"
    streams = client.query(sql)
    print(streams)
