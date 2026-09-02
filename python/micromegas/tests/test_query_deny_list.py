"""End-to-end tests for the admin-managed query deny list
(`tasks/query_deny_list_plan.md`), against `local_test_env`.

`local_test_env` runs with auth disabled, so every caller here is an admin -- there is no
non-admin path to exercise from this client.

These tests find their target's fingerprint the same way the plan's incident runbook describes:
run the query once, look it up in the audit log by a distinctive marker in its SQL text, and
read `sql_hash` back out of the audit record -- rather than duplicating the Rust-side
`fingerprint_of` algorithm in Python.
"""

import uuid

import pandas
import pyarrow
import pytest

from .otlp_helpers import assert_eventually
from .test_utils import client, begin, end


def _marker_sql(marker):
    # A distinctive, otherwise-inert query -- its own literal marker is what both the audit-log
    # lookup and (for the non-sql_hash tests) the deny rule itself key on.
    return f"SELECT 1 AS deny_test_marker_{marker}"


def _discover_sql_hash(marker, timeout_s=30):
    """Finds the `sql_hash` the server computed for the `_marker_sql(marker)` query, by polling
    the audit log -- it only lands once the telemetry sink flushes and the maintenance role
    materializes `log_entries` (comfortably above `MICROMEGAS_FLUSH_PERIOD`, 5s in
    `local_test_env`). Matches the audit record's `sql` field exactly: a LIKE on the marker
    would also match this poll query's own audit record (whose SQL quotes the marker as a
    literal), and once one poll's record materializes, ORDER BY time DESC would return the
    poll's fingerprint instead of the marker query's."""

    marker_sql = _marker_sql(marker)

    def query():
        sql = (
            "SELECT jsonb_as_string(jsonb_get(jsonb_parse(msg), 'sql_hash')) AS sql_hash "
            "FROM log_entries "
            "WHERE target = 'flightsql_query_audit' "
            f"AND jsonb_as_string(jsonb_get(jsonb_parse(msg), 'sql')) = '{marker_sql}' "
            "ORDER BY time DESC LIMIT 1"
        )
        return client.query(sql, begin, end)

    df = assert_eventually(
        query,
        lambda r: not r.empty,
        timeout_s=timeout_s,
        msg=f"waiting for an audit record for marker {marker}",
    )
    return str(df.iloc[0]["sql_hash"])


def _deny(match_expr, reason):
    """Calls `deny_queries`, returning the new rule's id."""
    escaped_expr = match_expr.replace("'", "''")
    escaped_reason = reason.replace("'", "''")
    sql = (
        f"SELECT msg AS rule_id FROM deny_queries('{escaped_expr}', '{escaped_reason}')"
    )
    df = client.query(sql)
    assert len(df) == 1
    return str(df.iloc[0]["rule_id"])


def _remove(rule_id):
    df = client.query(f"SELECT remove_query_denial('{rule_id}') AS result")
    return str(df.iloc[0]["result"])


def _list_rule_ids():
    df = client.query("SELECT rule_id FROM list_query_denials()")
    return set(df["rule_id"].astype(str))


@pytest.fixture
def marker():
    return uuid.uuid4().hex


def test_deny_by_sql_hash_then_remove_restores_access(marker):
    sql = _marker_sql(marker)
    # Baseline: the query succeeds and lands in the audit log.
    client.query(sql)
    sql_hash = _discover_sql_hash(marker)

    rule_id = _deny(f"sql_hash = '{sql_hash}'", "test: deny by sql_hash")
    try:
        with pytest.raises(pyarrow.lib.ArrowInvalid) as exc_info:
            client.query(sql)
        message = str(exc_info.value)
        assert rule_id in message
        assert "denied" in message.lower()
    finally:
        result = _remove(rule_id)
        assert result.startswith("SUCCESS"), result

    # The rule is gone: the same query succeeds again.
    client.query(sql)


def test_non_matching_query_is_unaffected_while_rule_stands(marker):
    other_marker = uuid.uuid4().hex
    sql = _marker_sql(marker)
    client.query(sql)
    sql_hash = _discover_sql_hash(marker)

    rule_id = _deny(f"sql_hash = '{sql_hash}'", "test: non-matching unaffected")
    try:
        # A different query, whose fingerprint the rule does not name, is unaffected.
        result = client.query(_marker_sql(other_marker))
        assert len(result) == 1
    finally:
        _remove(rule_id)


def test_denial_emits_a_warning_log_and_a_tagged_metric(marker):
    sql = _marker_sql(marker)
    client.query(sql)
    sql_hash = _discover_sql_hash(marker)

    rule_id = _deny(f"sql_hash = '{sql_hash}'", "test: denial signals")
    try:
        with pytest.raises(pyarrow.lib.ArrowInvalid):
            client.query(sql)

        def find_warn_row():
            log_sql = (
                "SELECT msg FROM log_entries "
                "WHERE level <= 3 AND msg LIKE 'query denied%' "
                f"AND msg LIKE '%{rule_id}%' "
                "ORDER BY time DESC LIMIT 1"
            )
            return client.query(log_sql, begin, end)

        warn_row = assert_eventually(
            find_warn_row,
            lambda r: not r.empty,
            msg="waiting for the denial warning log line",
        )
        assert rule_id in str(warn_row.iloc[0]["msg"])

        def find_metric_row():
            metric_sql = (
                "SELECT sum(value) AS denied FROM measures "
                "WHERE name = 'query_denied' "
                f"AND property_get(properties, 'rule_id') = '{rule_id}'"
            )
            return client.query(metric_sql, begin, end)

        metric_row = assert_eventually(
            find_metric_row,
            lambda r: not r.empty and r.iloc[0]["denied"] and r.iloc[0]["denied"] > 0,
            msg="waiting for the query_denied metric",
        )
        assert metric_row.iloc[0]["denied"] > 0
    finally:
        _remove(rule_id)


def test_no_column_expression_is_rejected():
    with pytest.raises(pyarrow.lib.ArrowInvalid) as exc_info:
        _deny("true", "test: no column")
    assert "column" in str(exc_info.value).lower()


def test_syntactically_invalid_expression_is_rejected():
    with pytest.raises(pyarrow.lib.ArrowInvalid):
        _deny("client = ", "test: invalid syntax")


def test_list_query_denials_shows_and_drops_the_rule(marker):
    sql = _marker_sql(marker)
    client.query(sql)
    sql_hash = _discover_sql_hash(marker)

    rule_id = _deny(f"sql_hash = '{sql_hash}'", "test: list visibility")
    assert rule_id in _list_rule_ids()

    _remove(rule_id)
    assert rule_id not in _list_rule_ids()


def test_last_hit_at_is_populated_after_a_refresh_tick(marker):
    sql = _marker_sql(marker)
    client.query(sql)
    sql_hash = _discover_sql_hash(marker)

    rule_id = _deny(f"sql_hash = '{sql_hash}'", "test: last_hit_at")
    try:
        with pytest.raises(pyarrow.lib.ArrowInvalid):
            client.query(sql)

        def find_last_hit():
            return client.query(
                f"SELECT last_hit_at FROM list_query_denials() WHERE rule_id = '{rule_id}'"
            )

        row = assert_eventually(
            find_last_hit,
            lambda r: not r.empty and pandas.notna(r.iloc[0]["last_hit_at"]),
            timeout_s=30,
            msg="waiting for last_hit_at to be flushed by a refresh tick",
        )
        assert pandas.notna(row.iloc[0]["last_hit_at"])
    finally:
        _remove(rule_id)


def test_admin_recovery_escape_hatch_survives_a_rule_matching_everything():
    # The python test client always sends client='python' -- a rule keyed on it matches every
    # query this test issues, including the recovery statements below, except that the
    # admin-recovery escape hatch exempts any statement naming
    # `remove_query_denial`/`deny_queries`/`list_query_denials` for an admin caller.
    rule_id = _deny("client = 'python'", "test: escape hatch")
    try:
        # list_query_denials itself mentions its own name, so it is exempt too.
        assert rule_id in _list_rule_ids()
    finally:
        result = _remove(rule_id)
        assert result.startswith("SUCCESS"), result
    assert rule_id not in _list_rule_ids()
