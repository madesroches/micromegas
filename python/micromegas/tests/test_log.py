from .test_utils import *

def test_log_query():
    sql = "select process_id from log_entries LIMIT 1;"
    rows = client.query_view("log_entries", "global", begin, end, sql)
    process_id = rows.iloc[0]["process_id"]

    sql = "select * from log_entries where process_id='{process_id}' LIMIT 1024;".format( process_id=process_id )
    log_entries = client.query_view("log_entries", "global", begin, end, sql)
    print(log_entries)

    sql = "select * from log_entries LIMIT 1024;"
    log_entries = client.query_view("log_entries", process_id, begin, end, sql)
    print(log_entries)
