"""End-to-end tests for DDL-defined eagerly materialized views, against
`local_test_env`.

`local_test_env` runs with auth disabled, so every caller here is an admin -- there is no
non-admin path to exercise from this client (see `rust/public/tests/read_policy_threading_tests.rs`
for that coverage). Relies on `start_services.py` exporting a low
`MICROMEGAS_VIEW_DEFINITION_REFRESH_SECONDS` (5s) so the daemon-pickup assertion below runs in
seconds rather than the 60s production default.
"""

import datetime
import uuid

import pyarrow
import pytest

from .otlp_helpers import assert_eventually
from .test_utils import client, end

# `begin` spans ~10000 days; materialize_partitions loops once per delta over that range, so
# tests that only need a few partitions materialize a narrow window near `end` instead.
mat_begin = end - datetime.timedelta(days=1)


def _unique_name(prefix):
    return f"{prefix}_{uuid.uuid4().hex[:8]}"


def _drop(name):
    client.query(f"DROP MATERIALIZED VIEW IF EXISTS {name}")


def _create_simple(name, update_group, source="log_entries"):
    """A minimal view over `log_entries`, aggregated by minute, carrying `audience` (check 2) and
    counting `blocks` (check 5)."""
    sql = f"""
    CREATE OR REPLACE MATERIALIZED VIEW {name} WITH (
      extract_query = $$
        SELECT date_bin('1 minute', time) as time_bin,
               arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience,
               count(*) as count
        FROM {source}
        WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
        GROUP BY time_bin
      $$,
      count_src_query = $$
        SELECT sum(nb_objects) as count FROM blocks
        WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
      $$,
      merge_partitions_query = $$
        SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience,
               sum(count) as count
        FROM {{source}} GROUP BY time_bin
      $$,
      update_group = {update_group},
      time_column = 'time_bin'
    )
    """
    return client.query(sql)


def test_create_or_replace_appears_in_listings_and_daemon_materializes():
    name = _unique_name("ddl_e2e")
    try:
        df = _create_simple(name, 9000)
        assert len(df) == 1
        assert df.iloc[0]["view_set_name"] == name
        assert df.iloc[0]["status"] == "created"

        view_sets = client.query("SELECT * FROM list_view_sets()")
        assert name in set(view_sets["view_set_name"])

        definitions = client.query("SELECT * FROM list_view_definitions()")
        assert name in set(definitions["view_set_name"])

        def has_partitions():
            return client.query(
                f"SELECT * FROM list_partitions() WHERE view_set_name = '{name}'"
            )

        # No materialize_partitions call here -- this is what proves the daemon itself picked
        # the new view set up, without a restart.
        assert_eventually(
            has_partitions,
            lambda r: not r.empty,
            timeout_s=60,
            msg=f"waiting for the daemon to pick up {name} without materialize_partitions",
        )
    finally:
        _drop(name)


def test_a_ddl_view_may_read_another(begin_=mat_begin, end_=end):
    a = _unique_name("ddl_e2e_a")
    b = _unique_name("ddl_e2e_b")
    try:
        _create_simple(a, 9000)
        # `b` reads `a` directly, filtering on `a`'s own event-time column (time_bin) rather than
        # insert_time -- `count_src_query` still counts `blocks`, never `a` itself (check 5).
        sql = f"""
        CREATE OR REPLACE MATERIALIZED VIEW {b} WITH (
          extract_query = $$
            SELECT time_bin, audience, count
            FROM {a}
            WHERE time_bin >= '{{begin}}' AND time_bin < '{{end}}'
          $$,
          count_src_query = $$
            SELECT sum(nb_objects) as count FROM blocks
            WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
          $$,
          merge_partitions_query = $$
            SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience,
                   sum(count) as count
            FROM {{source}} GROUP BY time_bin
          $$,
          update_group = 9001,
          time_column = 'time_bin'
        )
        """
        df = client.query(sql)
        assert df.iloc[0]["status"] == "created"

        client.query(
            f"SELECT * FROM materialize_partitions('{a}', TIMESTAMP '{begin_.isoformat()}', "
            f"TIMESTAMP '{end_.isoformat()}', 86400)"
        )
        client.query(
            f"SELECT * FROM materialize_partitions('{b}', TIMESTAMP '{begin_.isoformat()}', "
            f"TIMESTAMP '{end_.isoformat()}', 86400)"
        )

        a_rows = client.query(
            f"SELECT time_bin, sum(count) as count FROM {a} GROUP BY time_bin"
        )
        b_rows = client.query(
            f"SELECT time_bin, sum(count) as count FROM {b} GROUP BY time_bin"
        )
        assert set(a_rows["time_bin"]) == set(b_rows["time_bin"])
    finally:
        _drop(b)
        _drop(a)


def test_replace_with_a_different_schema_stops_reading_old_partitions():
    name = _unique_name("ddl_e2e_schema")
    try:
        _create_simple(name, 9000)
        client.query(
            f"SELECT * FROM materialize_partitions('{name}', TIMESTAMP '{mat_begin.isoformat()}', "
            f"TIMESTAMP '{end.isoformat()}', 86400)"
        )
        before = client.query(f"SELECT * FROM {name}")

        # A schema-changing replace: drops `audience`, adds a new column.
        sql = f"""
        CREATE OR REPLACE MATERIALIZED VIEW {name} WITH (
          extract_query = $$
            SELECT date_bin('1 minute', time) as time_bin,
                   arrow_cast(max(process_id), 'Dictionary(Int32, Utf8)') as process_id,
                   count(*) as count
            FROM log_entries
            WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
            GROUP BY time_bin
          $$,
          count_src_query = $$
            SELECT sum(nb_objects) as count FROM blocks
            WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
          $$,
          merge_partitions_query = $$
            SELECT time_bin, arrow_cast(max(process_id), 'Dictionary(Int32, Utf8)') as process_id,
                   sum(count) as count
            FROM {{source}} GROUP BY time_bin
          $$,
          update_group = 9000,
          time_column = 'time_bin'
        )
        """
        df = client.query(sql)
        assert df.iloc[0]["status"] == "replaced"

        # The old (audience-carrying) partitions are no longer read: the view now serves only
        # the new schema's rows -- since nothing has been re-materialized yet, that's zero rows.
        after = client.query(f"SELECT * FROM {name}")
        assert len(after) == 0
        assert (
            "audience" in before.columns
        )  # sanity: `before` was collected under the old schema
        assert "process_id" in after.columns
        assert "audience" not in after.columns
    finally:
        _drop(name)


def test_drop_removes_from_both_listings():
    a = _unique_name("ddl_e2e_drop_a")
    b = _unique_name("ddl_e2e_drop_b")
    try:
        _create_simple(a, 9000)
        sql = f"""
        CREATE OR REPLACE MATERIALIZED VIEW {b} WITH (
          extract_query = $$
            SELECT time_bin, audience, count FROM {a}
            WHERE time_bin >= '{{begin}}' AND time_bin < '{{end}}'
          $$,
          count_src_query = $$
            SELECT sum(nb_objects) as count FROM blocks
            WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
          $$,
          merge_partitions_query = $$
            SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience,
                   sum(count) as count
            FROM {{source}} GROUP BY time_bin
          $$,
          update_group = 9001,
          time_column = 'time_bin'
        )
        """
        client.query(sql)

        # Dropping in dependency order: b before a, the reverse of their creation order.
        df_b = client.query(f"DROP MATERIALIZED VIEW {b}")
        assert df_b.iloc[0]["status"] == "dropped"
        df_a = client.query(f"DROP MATERIALIZED VIEW {a}")
        assert df_a.iloc[0]["status"] == "dropped"

        view_sets = set(client.query("SELECT * FROM list_view_sets()")["view_set_name"])
        definitions = set(
            client.query("SELECT * FROM list_view_definitions()")["view_set_name"]
        )
        assert a not in view_sets and b not in view_sets
        assert a not in definitions and b not in definitions
    finally:
        _drop(b)
        _drop(a)


def test_dropping_a_view_another_reads_is_refused():
    a = _unique_name("ddl_e2e_protect_a")
    b = _unique_name("ddl_e2e_protect_b")
    try:
        _create_simple(a, 9000)
        sql = f"""
        CREATE OR REPLACE MATERIALIZED VIEW {b} WITH (
          extract_query = $$
            SELECT time_bin, audience, count FROM {a}
            WHERE time_bin >= '{{begin}}' AND time_bin < '{{end}}'
          $$,
          count_src_query = $$
            SELECT sum(nb_objects) as count FROM blocks
            WHERE insert_time >= '{{begin}}' AND insert_time < '{{end}}'
          $$,
          merge_partitions_query = $$
            SELECT time_bin, arrow_cast(max(audience), 'Dictionary(Int32, Utf8)') as audience,
                   sum(count) as count
            FROM {{source}} GROUP BY time_bin
          $$,
          update_group = 9001,
          time_column = 'time_bin'
        )
        """
        client.query(sql)

        with pytest.raises(pyarrow.lib.ArrowInvalid) as exc_info:
            client.query(f"DROP MATERIALIZED VIEW {a}")
        assert b in str(exc_info.value)
    finally:
        _drop(b)
        _drop(a)
