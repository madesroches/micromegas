from .test_utils import *


def test_measures_query():
    sql = "select process_id from measures LIMIT 1;"
    rows = client.query(sql)
    process_id = rows.iloc[0]["process_id"]

    sql = "select * from measures where process_id='{process_id}' LIMIT 1024;".format(
        process_id=process_id
    )
    measures = client.query(sql)
    print(measures)

    sql = "select * from measures LIMIT 1024;"
    measures = client.query(sql)
    print(measures)
