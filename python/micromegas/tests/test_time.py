import micromegas
import pytest


def test_format_string():
    assert "2024-08-26T17:32:00+00:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00.000+00:00"
    )

    # missing time zone
    with pytest.raises(RuntimeError) as e_info:
        assert "2024-08-26T17:32:00+00:00" == micromegas.time.format_datetime(
            "2024-08-26T17:32:00"
        )


def test_format_datetime_z_suffix():
    assert "2024-08-26T17:32:00+00:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00Z"
    )


def test_format_datetime_lowercase_z_suffix():
    assert "2024-08-26T17:32:00+00:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00z"
    )


def test_format_datetime_non_utc_offset_preserved():
    assert "2024-08-26T17:32:00-05:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00-05:00"
    )


def test_parse_datetime_invalid_raises_value_error():
    with pytest.raises(ValueError):
        micromegas.time.parse_datetime("not-a-timestamp")


def test_parse_datetime_fractional_seconds_z():
    dt = micromegas.time.parse_datetime("2024-08-26T17:32:00.123456Z")
    assert dt.microsecond == 123456
    assert dt.tzinfo is not None


def test_format_datetime_single_digit_fraction_z():
    assert "2024-08-26T17:32:00.500000+00:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00.5Z"
    )


def test_format_datetime_truncates_beyond_microsecond_z():
    assert "2024-08-26T17:32:00.123456+00:00" == micromegas.time.format_datetime(
        "2024-08-26T17:32:00.1234567Z"
    )
