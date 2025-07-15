from .test_utils import *
import json


def test_jsonb_parse():
    sql = """
      SELECT jsonb_parse('{ "name" : "value" }') as json_bin
    """
    res = client.query(sql)
    json_bin = res.iloc[0]["json_bin"]
    assert json_bin == b"@\x00\x00\x01\x10\x00\x00\x04\x10\x00\x00\x05namevalue"


def test_jsonb_format_json():
    sql = """
      SELECT jsonb_format_json(jsonb_parse('{ "name" : "value" }')) as json_string
    """
    res = client.query(sql)
    json_string = res.iloc[0]["json_string"]
    assert json_string == '{"name":"value"}'


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
