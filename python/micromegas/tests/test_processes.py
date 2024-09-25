from .test_utils import *


def test_processes_query():
    sql = "select * from processes LIMIT 10;"
    processes = client.query(sql)
    print(processes)
