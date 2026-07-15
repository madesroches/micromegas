"""Helpers for the OTLP/HTTP end-to-end tests.

The ingest path derives process_id from a hash of many resource attributes
(`rust/otel-ingestion/src/identity.rs`). Rather than mirror that formula
client-side — which silently drifts whenever the Rust side adds an identity
field — the tests tag each run with a unique `service.instance.id` and look up
the server-assigned process_id after ingestion (see `discover_process_id`).
"""

import time


def discover_process_id(client, instance_id, begin, end, timeout_s=60):
    """Return the server-assigned process_id for a given `service.instance.id`.

    The OTLP ingest path stores resource attributes as process properties
    prefixed with `otel.resource.`, so a run's unique instance id is queryable
    as `otel.resource.service.instance.id`. Polls until the process row is
    materialized (it lands within a second or two of ingestion).
    """
    return discover_process_id_by_property(
        client,
        "otel.resource.service.instance.id",
        instance_id,
        begin,
        end,
        timeout_s=timeout_s,
    )


def discover_process_id_by_property(
    client, prop_key, prop_value, begin, end, timeout_s=60
):
    """Return the server-assigned process_id for a given process property.

    Every resource attribute lands in `processes.properties` under
    `otel.resource.*` (`ProcessFromResource::build`). Useful when the request
    can't set `service.instance.id` (e.g. webhook headers) — tag the run with
    a unique value on some other attribute instead and look it up here.
    """

    def query():
        sql = (
            "SELECT process_id FROM processes "
            f"WHERE property_get(properties, '{prop_key}') = '{prop_value}'"
        )
        return client.query(sql, begin, end)

    df = assert_eventually(
        query,
        lambda r: not r.empty,
        timeout_s=timeout_s,
        msg=f"waiting for process with {prop_key}={prop_value}",
    )
    return str(df.iloc[0]["process_id"])


def assert_eventually(query_fn, predicate, timeout_s=30, interval_s=0.5, msg=None):
    """Poll `query_fn()` until `predicate(result)` returns truthy.

    JIT views materialize on first query against a process_id and can take a
    moment to settle, so e2e assertions need to retry. Returns the final
    successful query result.
    """
    deadline = time.monotonic() + timeout_s
    last = None
    last_exc = None
    while time.monotonic() < deadline:
        try:
            last = query_fn()
            if predicate(last):
                return last
        except Exception as e:
            last_exc = e
        time.sleep(interval_s)
    if last_exc is not None:
        raise AssertionError(
            f"assert_eventually timed out after {timeout_s}s: {msg or ''} "
            f"(last exception: {last_exc!r})"
        )
    raise AssertionError(
        f"assert_eventually timed out after {timeout_s}s: {msg or ''} "
        f"(last result: {last!r})"
    )


# Helpers for building OTLP proto messages from primitives. Centralized so
# each test reads as a 3-line emit-then-assert rather than 30 lines of proto
# scaffolding.

from opentelemetry.proto.common.v1 import common_pb2
from opentelemetry.proto.resource.v1 import resource_pb2


def string_kv(key, value):
    """KeyValue with a string value."""
    return common_pb2.KeyValue(
        key=key,
        value=common_pb2.AnyValue(string_value=value),
    )


def int_kv(key, value):
    """KeyValue with an int value."""
    return common_pb2.KeyValue(
        key=key,
        value=common_pb2.AnyValue(int_value=value),
    )


def make_resource(attrs):
    """Build a Resource from a dict of attributes (string values only)."""
    return resource_pb2.Resource(
        attributes=[string_kv(k, str(v)) for k, v in attrs.items()],
    )
