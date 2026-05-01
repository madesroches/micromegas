"""Helpers for the OTLP/HTTP end-to-end tests.

The Rust ingest path computes process_id deterministically from a hash of
resource attributes (`rust/otel-ingestion/src/identity.rs`). To verify the
data lands queryable for a known producer, the test must compute the same
process_id client-side. This module is a Python port of that formula.

If the Rust formula ever changes (which would require bumping
NS_OTEL_PROCESS_V1 to _V2), this module has to change in lockstep.
"""

import time
import uuid

# Namespace UUIDs are load-bearing — must match
# `rust/otel-ingestion/src/identity.rs` exactly.
NS_OTEL_PROCESS_V1 = uuid.UUID("80a447b8-fcdd-42a6-a613-f6c8719cd5fe")
NS_OTEL_STREAM_V1 = uuid.UUID("fe93bacf-e851-4cf6-8526-05f8454b3488")
NS_OTEL_BLOCK_V1 = uuid.UUID("5829a6f7-0577-4c8c-862f-cf4fdab445cc")

# ASCII unit separator between concatenated string fields.
SEPARATOR = "\x1f"


def _norm(s):
    """trim + lower-case (matches the Rust `norm` helper)."""
    return (s or "").strip().lower()


def compute_otel_process_id(
    host_name="",
    host_id="",
    pid="",
    start_time="",
    service_namespace="",
    service_name="",
    instance_id="",
):
    """Mirror of `process_id_from_resource` in identity.rs.

    Field order is the contract — changing it requires a new namespace UUID.
    `pid` and `start_time` are passed through verbatim (no case folding);
    everything else is trim+lower-cased.
    """
    key = SEPARATOR.join(
        [
            _norm(host_id),
            _norm(host_name),
            str(pid) if pid != "" else "",
            start_time or "",
            _norm(service_namespace),
            _norm(service_name),
            _norm(instance_id),
        ]
    )
    return uuid.uuid5(NS_OTEL_PROCESS_V1, key)


def compute_otel_stream_id(process_id, signal):
    """Mirror of `stream_id_from_process_signal` in identity.rs.

    `signal` must be one of "logs", "metrics", "traces".
    """
    if signal not in ("logs", "metrics", "traces"):
        raise ValueError(f"signal must be logs|metrics|traces, got {signal!r}")
    key = f"{process_id}{SEPARATOR}{signal}"
    return uuid.uuid5(NS_OTEL_STREAM_V1, key)


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
