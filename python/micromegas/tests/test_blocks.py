from .test_utils import *


def test_blocks_query():
    sql = "select * from blocks LIMIT 10;"
    blocks = client.query(sql)
    print(blocks)
