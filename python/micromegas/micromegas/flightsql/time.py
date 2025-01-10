import datetime
import pandas

def format_datetime(value):
    nonetype = type(None)
    value_type = type(value)
    if value_type == datetime.datetime:
        if value.tzinfo is None:
            raise RuntimeError("datetime needs a valid time zone")
        return value.isoformat()
    elif value_type == pandas.Timestamp:
        return value.isoformat()
    elif value_type == str:
        return format_datetime(datetime.datetime.fromisoformat(value))
    elif value_type == type(None):
        return None
    raise RuntimeError("value of unknown type in format_datetime")
