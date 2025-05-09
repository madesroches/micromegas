from .test_utils import *


def test_histo():
    sql = "select name, process_id from measures LIMIT 1;"
    rows = client.query(sql)
    process_id = rows.iloc[0]["process_id"]
    name = rows.iloc[0]["name"]

    print(name, process_id)
    sql = "select make_histogram(0.0, 100.0, 1000, value) from measures where process_id='{process_id}' AND name='{name}';".format(
        process_id=process_id, name=name
    )
    res = client.query(sql)
    print(res)
