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
