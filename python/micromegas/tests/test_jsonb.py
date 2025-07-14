from .test_utils import *


def test_jsonb_parse():
    sql = """
      SELECT jsonb_parse('{ "name" : "value" }') as json_bin
    """
    res = client.query(sql)
    json_bin = res.iloc[0]["json_bin"]
    assert json_bin == b"@\x00\x00\x01\x10\x00\x00\x04\x10\x00\x00\x05namevalue"


def test_jsonb_to_string():
    sql = """
      SELECT jsonb_to_string(jsonb_parse('{ "name" : "value" }')) as json_string
    """
    res = client.query(sql)
    json_string = res.iloc[0]["json_string"]
    assert json_string == '{"name":"value"}'
