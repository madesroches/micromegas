from .test_utils import *


def test_processes_query():
    sql = "select * from processes LIMIT 10;"
    processes = client.query(sql)
    print(processes)

def test_processes_properties_query():
    sql = "select properties, property_get(properties, 'build-version') from processes WHERE array_length(properties) > 0 LIMIT 10;"
    processes = client.query(sql)
    print(processes)
