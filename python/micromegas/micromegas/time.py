import datetime
import pandas
import re

def format_datetime(value):
    nonetype = type(None)
    match type(value):
        case datetime.datetime:
            if value.tzinfo is None:
                raise RuntimeError("datetime needs a valid time zone")
            return value.isoformat()
        case pandas.Timestamp:
            return value.isoformat()
        case nonetype:
            return None
    raise RuntimeError("value of unknown type in format_datetime")

def parse_time_delta(user_string):
    parser = re.compile("(\\d+)([mhd])")
    m = parser.match(user_string)
    nbr = int(m.group(1))
    unit = m.group(2)
    if unit == "m":
        return datetime.timedelta(minutes=nbr)
    elif unit == "h":
        return datetime.timedelta(hours=nbr)
    elif unit == "d":
        return datetime.timedelta(days=nbr)
    else:
        raise RuntimeError("invalid time delta: " + user_string)
