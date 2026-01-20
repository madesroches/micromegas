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


def test_expand_histogram():
    """Test expand_histogram table function via full SQL path."""
    sql = "select name, process_id from measures LIMIT 1;"
    rows = client.query(sql)
    process_id = rows.iloc[0]["process_id"]
    name = rows.iloc[0]["name"]

    sql = """
    SELECT bin_center, count
    FROM expand_histogram(
        (SELECT make_histogram(0.0, 100.0, 10, value)
         FROM measures
         WHERE process_id='{process_id}' AND name='{name}')
    )
    """.format(
        process_id=process_id, name=name
    )

    res = client.query(sql)
    print(res)

    # Verify structure
    assert "bin_center" in res.columns
    assert "count" in res.columns
    assert len(res) == 10  # 10 bins

    # Verify bin centers are evenly spaced
    bin_centers = res["bin_center"].tolist()
    expected_centers = [5.0, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0, 85.0, 95.0]
    for actual, expected in zip(bin_centers, expected_centers):
        assert abs(actual - expected) < 0.001, f"Expected {expected}, got {actual}"
