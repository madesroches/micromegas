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
