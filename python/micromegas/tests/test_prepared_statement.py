from .test_utils import *

def test_prepared_statement():
    sql = "select count(*) from log_entries"
    prepared_statement = client.prepare_statement(sql)
    print(prepared_statement)
