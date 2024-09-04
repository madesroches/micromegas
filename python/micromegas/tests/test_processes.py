from .test_utils import *


def test_processes_query():
    processes = client.query_processes(begin, end, limit)
    print(processes)

    sql = "select * from processes LIMIT 1024;"
    processes = client.query_view("processes", "global", begin, end, sql)
    print(processes)
