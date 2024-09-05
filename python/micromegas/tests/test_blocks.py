from .test_utils import *


def test_blocks_query():
    sql = "select * from blocks LIMIT 1024;"
    blocks = client.query_view("blocks", "global", begin, end, sql)
    print(blocks)
