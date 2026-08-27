from .test_utils import *
import json


def test_jsonb_parse():
    sql = """
      SELECT jsonb_parse('{ "name" : "value" }') as json_bin
    """
    res = client.query(sql)
    json_bin = res.iloc[0]["json_bin"]
    assert json_bin == b"@\x00\x00\x01\x10\x00\x00\x04\x10\x00\x00\x05namevalue"


def test_jsonb_parse_error():
    sql = """
      SELECT jsonb_parse('{ not json... }') as json_bin
    """
    res = client.query(sql)
    json_bin = res.iloc[0]["json_bin"]
    assert json_bin is None


def test_jsonb_format_json():
    sql = """
      SELECT jsonb_format_json(jsonb_parse('{ "name" : "value" }')) as json_string
    """
    res = client.query(sql)
    json_string = res.iloc[0]["json_string"]
    assert json_string == '{"name":"value"}'


def test_jsonb_format_json_error():
    sql = """
      SELECT jsonb_format_json(jsonb_parse('{ test }')) as json_string
    """
    res = client.query(sql)
    json_string = res.iloc[0]["json_string"]
    assert json_string is None


def test_jsonb_get():
    sql = """
      SELECT jsonb_format_json( jsonb_get( jsonb_parse('{ "name" : "value" }'), 'name' ) ) as value
    """
    res = client.query(sql)
    value = res.iloc[0]["value"]
    parsed = json.loads(value)
    assert parsed == "value"


def test_jsonb_cast_string():
    sql = """
      SELECT jsonb_as_string( jsonb_get( jsonb_parse('{ "name" : "value" }'), 'name' ) ) as value
    """
    res = client.query(sql)
    value = res.iloc[0]["value"]
    assert value == "value"


def test_jsonb_cast_f64():
    sql = """
      SELECT jsonb_as_f64( jsonb_get( jsonb_parse('{ "name" : 2.3 }'), 'name' ) ) as value
    """
    res = client.query(sql)
    value = res.iloc[0]["value"]
    assert value == 2.3


def test_jsonb_cast_i64():
    sql = """
      SELECT jsonb_as_i64( jsonb_get( jsonb_parse('{ "name" : 321321321321 }'), 'name' ) ) as value
    """
    res = client.query(sql)
    value = res.iloc[0]["value"]
    assert value == 321321321321


def test_jsonb_entries_per_row_expansion_over_properties():
    # unnest(jsonb_entries(properties)) over a real, dictionary-encoded `properties` column —
    # confirms per-row expansion through the dictionary fast path against genuine ingested data,
    # not a hand-built column (the shape/NULL/error edge cases are covered by the Rust suite).
    # `processes.properties` is used rather than `log_entries.properties` because per-log-entry
    # properties are typically empty; process-level properties (exe, username, ...) are not.
    sql = """
      SELECT process_id, kv['key'] as key, count(*) as n
      FROM (SELECT process_id, unnest(jsonb_entries(properties)) as kv FROM processes)
      GROUP BY process_id, key
      LIMIT 20
    """
    res = client.query(sql, begin, end)
    assert not res.empty
    assert (res["n"] > 0).all()
    assert res["key"].notna().all()


def test_jsonb_path_elements_nested_array():
    # unnest(jsonb_path_elements(...)) over a JSON body shaped like a real OTLP/webhook payload
    # with a nested array (see the `commits` array assertion in test_otlp_e2e.py), confirming the
    # path-elements UDF end to end over FlightSQL.
    sql = """
      SELECT jsonb_as_string(jsonb_get(commit, 'id')) as commit_id
      FROM (
        SELECT unnest(jsonb_path_elements(
          jsonb_parse('{"object_kind": "push", "commits": [{"id": "abc123"}, {"id": "def456"}]}'),
          '$.commits[*]'
        )) as commit
      )
      ORDER BY commit_id
    """
    res = client.query(sql)
    assert list(res["commit_id"]) == ["abc123", "def456"]
