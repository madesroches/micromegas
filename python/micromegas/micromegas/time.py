"""Time utility functions for Micromegas Python client.

This module provides utilities for formatting and parsing time values
used in queries and API calls.
"""

import datetime
import pandas
import re


def format_datetime(value):
    """Format various datetime types into RFC3339/ISO8601 strings for queries.

    Converts Python datetime objects, pandas Timestamps, or datetime strings
    into a standardized RFC3339 format that the Micromegas server expects.
    Ensures all datetime values have timezone information.

    Args:
        value: The datetime value to format. Can be:
            - datetime.datetime: Must be timezone-aware
            - pandas.Timestamp: Will use its timezone information
            - str: ISO format string that will be parsed and reformatted
            - None: Returns None without modification

    Returns:
        str: RFC3339/ISO8601 formatted datetime string (e.g., "2024-01-01T12:00:00+00:00")
        None: If input value is None

    Raises:
        RuntimeError: If datetime is missing timezone information or value type is unsupported.

    Example:
        >>> import datetime
        >>> from datetime import timezone
        >>>
        >>> # Format timezone-aware datetime
        >>> dt = datetime.datetime(2024, 1, 1, 12, 0, 0, tzinfo=timezone.utc)
        >>> format_datetime(dt)
        '2024-01-01T12:00:00+00:00'
        >>>
        >>> # Format pandas Timestamp
        >>> import pandas as pd
        >>> ts = pd.Timestamp('2024-01-01 12:00:00', tz='UTC')
        >>> format_datetime(ts)
        '2024-01-01T12:00:00+00:00'
        >>>
        >>> # Parse and format string
        >>> format_datetime('2024-01-01T12:00:00Z')
        '2024-01-01T12:00:00+00:00'

    Note:
        Always use timezone-aware datetime objects to avoid ambiguity.
        The server requires RFC3339 format for all time-based queries.
    """
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


def parse_time_delta(user_string):
    """Parse human-readable time delta strings into timedelta objects.

    Converts simple time duration strings like "1h", "30m", or "7d" into
    Python timedelta objects for use in time calculations.

    Args:
        user_string (str): Time delta string with format "<number><unit>" where:
            - number: Positive integer
            - unit: 'm' for minutes, 'h' for hours, 'd' for days

    Returns:
        datetime.timedelta: The parsed time duration.

    Raises:
        RuntimeError: If the string format is invalid or uses unsupported units.

    Example:
        >>> # Parse various time deltas
        >>> parse_time_delta('30m')  # 30 minutes
        datetime.timedelta(seconds=1800)
        >>>
        >>> parse_time_delta('2h')   # 2 hours
        datetime.timedelta(seconds=7200)
        >>>
        >>> parse_time_delta('7d')   # 7 days
        datetime.timedelta(days=7)
        >>>
        >>> # Use in time calculations
        >>> import datetime
        >>> now = datetime.datetime.now(datetime.timezone.utc)
        >>> one_hour_ago = now - parse_time_delta('1h')

    Supported Units:
        - 'm': minutes
        - 'h': hours
        - 'd': days

    Note:
        For more complex time expressions, use datetime.timedelta directly.
        This function is designed for simple, common time durations.
    """
    parser = re.compile(r"(\d+)([mhd])")
    m = parser.match(user_string)
    if not m:
        raise RuntimeError(
            f"invalid time delta format: '{user_string}'. Expected format: '<number><unit>' where unit is m/h/d"
        )
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
