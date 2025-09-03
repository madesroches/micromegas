from .test_utils import *


def test_log_query():
    sql = "select process_id from log_entries LIMIT 1;"
    rows = client.query(sql, begin, end)
    process_id = rows.iloc[0]["process_id"]

    sql = "select * from log_entries where process_id='{process_id}' LIMIT 10;".format(
        process_id=process_id
    )
    log_entries = client.query(sql, begin, end)
    print(log_entries)

    sql = "select * from log_entries LIMIT 10;"
    log_entries = client.query(sql, begin, end)
    print(log_entries)


def test_implicit_log_query():
    # query with no specified time range
    rows = client.query("SELECT COUNT(*) FROM log_entries;")
    print(rows)
