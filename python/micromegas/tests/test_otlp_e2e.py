"""End-to-end test of the OTLP/HTTP ingestion path.

Builds OTLP proto requests by hand (no OTel SDK exporter machinery — that
would batch on a background thread and need a `force_flush()` + sleep, which
is flaky in CI). Each test POSTs a self-contained payload to the running
ingestion server and queries the lakehouse via FlightSQL to assert the rows
landed.

Assumes services are already running:
    python3 local_test_env/ai_scripts/start_services.py
"""

import datetime
import os
import time
import uuid

import requests
from opentelemetry.proto.collector.logs.v1 import logs_service_pb2
from opentelemetry.proto.collector.metrics.v1 import metrics_service_pb2
from opentelemetry.proto.collector.trace.v1 import trace_service_pb2
from opentelemetry.proto.logs.v1 import logs_pb2
from opentelemetry.proto.metrics.v1 import metrics_pb2
from opentelemetry.proto.trace.v1 import trace_pb2

from .otlp_helpers import (
    assert_eventually,
    compute_otel_process_id,
    make_resource,
    string_kv,
)
from .test_utils import client


INGESTION_URL = os.environ.get("MICROMEGAS_INGESTION_URL", "http://127.0.0.1:9000")
LOGS_ENDPOINT = f"{INGESTION_URL}/ingestion/otlp/v1/logs"
METRICS_ENDPOINT = f"{INGESTION_URL}/ingestion/otlp/v1/metrics"
TRACES_ENDPOINT = f"{INGESTION_URL}/ingestion/otlp/v1/traces"

PROTOBUF_HEADERS = {"Content-Type": "application/x-protobuf"}


def _now_ns():
    """Current wall-clock time as nanoseconds since the Unix epoch."""
    return time.time_ns()


def _fresh_resource_attrs():
    """Build a resource attribute set with a per-run-unique service.instance.id.

    Different tests / runs get distinct process_ids without needing a DB wipe
    between runs. Returns (attrs_dict, expected_process_id).
    """
    instance_id = str(uuid.uuid4())
    host_name = "otlp-e2e-host"
    host_id = "otlp-e2e-host-id"
    pid = "12345"
    attrs = {
        "service.name": "otlp-e2e",
        "service.instance.id": instance_id,
        "host.name": host_name,
        "host.id": host_id,
        "process.pid": pid,
    }
    expected_pid = compute_otel_process_id(
        host_name=host_name,
        host_id=host_id,
        pid=pid,
        service_name="otlp-e2e",
        instance_id=instance_id,
    )
    return attrs, expected_pid


def _query_window():
    """A wide [begin, end] window centered on now — covers JIT + clock skew."""
    now = datetime.datetime.now(datetime.timezone.utc)
    return now - datetime.timedelta(hours=1), now + datetime.timedelta(hours=1)


# ---------------------------------------------------------------------------
# Logs
# ---------------------------------------------------------------------------


def _build_logs_request(resource_attrs, base_ns):
    """5 records spanning 3 severity levels (INFO/ERROR/FATAL).

    severity_number values exercise the level collapse:
      9  → Info  (4)
      17 → Error (2)
      22 → Fatal (1)
    """
    records = []
    severities = [9, 9, 17, 17, 22]
    for i, sev in enumerate(severities):
        records.append(
            logs_pb2.LogRecord(
                time_unix_nano=base_ns + i,
                observed_time_unix_nano=base_ns + i,
                severity_number=sev,
                severity_text="INFO" if sev == 9 else "ERROR" if sev == 17 else "FATAL",
                body=common_any_string(f"e2e log {i} sev={sev}"),
                attributes=[string_kv("seq", str(i))],
            )
        )
    return logs_service_pb2.ExportLogsServiceRequest(
        resource_logs=[
            logs_pb2.ResourceLogs(
                resource=make_resource(resource_attrs),
                scope_logs=[
                    logs_pb2.ScopeLogs(
                        scope=common_scope("e2e.logs", "1.0.0"),
                        log_records=records,
                    )
                ],
            )
        ]
    )


def common_any_string(value):
    """Helper: AnyValue with a string body."""
    from opentelemetry.proto.common.v1 import common_pb2

    return common_pb2.AnyValue(string_value=value)


def common_scope(name, version=""):
    from opentelemetry.proto.common.v1 import common_pb2

    return common_pb2.InstrumentationScope(name=name, version=version)


def test_otlp_logs_e2e():
    attrs, expected_pid = _fresh_resource_attrs()
    base_ns = _now_ns()
    req = _build_logs_request(attrs, base_ns)

    resp = requests.post(
        LOGS_ENDPOINT,
        data=req.SerializeToString(),
        headers=PROTOBUF_HEADERS,
        timeout=10,
    )
    assert resp.status_code == 200, resp.text
    assert resp.headers.get("content-type", "").startswith("application/x-protobuf")

    begin, end = _query_window()
    pid_str = str(expected_pid)

    def query_count():
        sql = (
            f"SELECT count(*) AS c FROM log_entries " f"WHERE process_id = '{pid_str}'"
        )
        return client.query(sql, begin, end)

    df = assert_eventually(
        query_count,
        lambda r: not r.empty and int(r.iloc[0]["c"]) >= 5,
        timeout_s=60,
        msg=f"waiting for 5 log_entries with process_id={pid_str}",
    )
    assert int(df.iloc[0]["c"]) >= 5

    sql = (
        "SELECT level, msg, "
        "  jsonb_as_string(jsonb_get(properties, 'otel.scope.name')) AS scope_name, "
        "  jsonb_as_string(jsonb_get(properties, 'otel.severity_text')) AS severity_text "
        f"FROM log_entries WHERE process_id = '{pid_str}' "
        "ORDER BY time"
    )
    rows = client.query(sql, begin, end)
    assert len(rows) >= 5
    levels = list(rows["level"][:5])
    # Severity 9 → 4 (Info), 17 → 2 (Error), 22 → 1 (Fatal)
    assert levels == [4, 4, 2, 2, 1], levels
    msgs = list(rows["msg"][:5])
    for i, msg in enumerate(msgs):
        assert msg == f"e2e log {i} sev={[9, 9, 17, 17, 22][i]}", (i, msg)
    assert rows["scope_name"].iloc[0] == "e2e.logs"
    assert rows["severity_text"].iloc[0] == "INFO"


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------


def _build_metrics_request(resource_attrs, base_ns):
    """One Sum + one Gauge under a shared resource."""
    sum_metric = metrics_pb2.Metric(
        name="e2e.requests",
        unit="1",
        sum=metrics_pb2.Sum(
            data_points=[
                metrics_pb2.NumberDataPoint(
                    time_unix_nano=base_ns,
                    start_time_unix_nano=base_ns,
                    as_int=42,
                )
            ],
            aggregation_temporality=metrics_pb2.AggregationTemporality.AGGREGATION_TEMPORALITY_CUMULATIVE,
            is_monotonic=True,
        ),
    )
    gauge_metric = metrics_pb2.Metric(
        name="e2e.queue_depth",
        unit="items",
        gauge=metrics_pb2.Gauge(
            data_points=[
                metrics_pb2.NumberDataPoint(
                    time_unix_nano=base_ns + 1,
                    as_double=3.5,
                )
            ],
        ),
    )
    return metrics_service_pb2.ExportMetricsServiceRequest(
        resource_metrics=[
            metrics_pb2.ResourceMetrics(
                resource=make_resource(resource_attrs),
                scope_metrics=[
                    metrics_pb2.ScopeMetrics(
                        scope=common_scope("e2e.metrics", "1.0.0"),
                        metrics=[sum_metric, gauge_metric],
                    )
                ],
            )
        ]
    )


def test_otlp_metrics_e2e():
    attrs, expected_pid = _fresh_resource_attrs()
    base_ns = _now_ns()
    req = _build_metrics_request(attrs, base_ns)

    resp = requests.post(
        METRICS_ENDPOINT,
        data=req.SerializeToString(),
        headers=PROTOBUF_HEADERS,
        timeout=10,
    )
    assert resp.status_code == 200, resp.text

    begin, end = _query_window()
    pid_str = str(expected_pid)

    def query_count():
        sql = f"SELECT count(*) AS c FROM measures " f"WHERE process_id = '{pid_str}'"
        return client.query(sql, begin, end)

    df = assert_eventually(
        query_count,
        lambda r: not r.empty and int(r.iloc[0]["c"]) >= 2,
        timeout_s=60,
        msg=f"waiting for 2 measures with process_id={pid_str}",
    )
    assert int(df.iloc[0]["c"]) >= 2

    sql = (
        "SELECT name, unit, value, "
        "  jsonb_as_string(jsonb_get(properties, 'otel.metric.kind')) AS kind "
        f"FROM measures WHERE process_id = '{pid_str}' "
        "ORDER BY name"
    )
    rows = client.query(sql, begin, end)
    by_name = {r["name"]: r for _, r in rows.iterrows()}
    assert "e2e.queue_depth" in by_name
    assert "e2e.requests" in by_name
    qd = by_name["e2e.queue_depth"]
    rq = by_name["e2e.requests"]
    assert qd["unit"] == "items"
    assert qd["value"] == 3.5
    assert rq["unit"] == "1"
    assert rq["value"] == 42.0
    assert qd["kind"] == "gauge"
    assert rq["kind"] == "sum"


# ---------------------------------------------------------------------------
# Traces
# ---------------------------------------------------------------------------


def _build_traces_request(resource_attrs, base_ns):
    """A 3-span trace: root + 2 children."""
    trace_id = uuid.uuid4().bytes  # 16 bytes
    root_span_id = uuid.uuid4().bytes[:8]
    child1_span_id = uuid.uuid4().bytes[:8]
    child2_span_id = uuid.uuid4().bytes[:8]

    root = trace_pb2.Span(
        trace_id=trace_id,
        span_id=root_span_id,
        name="root",
        kind=trace_pb2.Span.SpanKind.SPAN_KIND_SERVER,
        start_time_unix_nano=base_ns,
        end_time_unix_nano=base_ns + 1_000_000,  # 1 ms
        status=trace_pb2.Status(
            code=trace_pb2.Status.StatusCode.STATUS_CODE_OK,
        ),
    )
    child1 = trace_pb2.Span(
        trace_id=trace_id,
        span_id=child1_span_id,
        parent_span_id=root_span_id,
        name="child-a",
        kind=trace_pb2.Span.SpanKind.SPAN_KIND_INTERNAL,
        start_time_unix_nano=base_ns + 100_000,
        end_time_unix_nano=base_ns + 500_000,
        status=trace_pb2.Status(
            code=trace_pb2.Status.StatusCode.STATUS_CODE_OK,
        ),
    )
    child2 = trace_pb2.Span(
        trace_id=trace_id,
        span_id=child2_span_id,
        parent_span_id=root_span_id,
        name="child-b",
        kind=trace_pb2.Span.SpanKind.SPAN_KIND_CLIENT,
        start_time_unix_nano=base_ns + 600_000,
        end_time_unix_nano=base_ns + 900_000,
        status=trace_pb2.Status(
            code=trace_pb2.Status.StatusCode.STATUS_CODE_ERROR,
            message="boom",
        ),
    )
    req = trace_service_pb2.ExportTraceServiceRequest(
        resource_spans=[
            trace_pb2.ResourceSpans(
                resource=make_resource(resource_attrs),
                scope_spans=[
                    trace_pb2.ScopeSpans(
                        scope=common_scope("e2e.traces", "1.0.0"),
                        spans=[root, child1, child2],
                    )
                ],
            )
        ]
    )
    return req, trace_id, root_span_id


def test_otlp_traces_e2e():
    attrs, expected_pid = _fresh_resource_attrs()
    base_ns = _now_ns()
    req, trace_id, root_span_id = _build_traces_request(attrs, base_ns)

    resp = requests.post(
        TRACES_ENDPOINT,
        data=req.SerializeToString(),
        headers=PROTOBUF_HEADERS,
        timeout=10,
    )
    assert resp.status_code == 200, resp.text

    begin, end = _query_window()
    pid_str = str(expected_pid)

    def query_count():
        sql = f"SELECT count(*) AS c FROM view_instance('otel_spans', '{pid_str}')"
        return client.query(sql, begin, end)

    df = assert_eventually(
        query_count,
        lambda r: not r.empty and int(r.iloc[0]["c"]) >= 3,
        timeout_s=60,
        msg=f"waiting for 3 spans with process_id={pid_str}",
    )

    sql = (
        "SELECT name, kind, status, status_message, "
        "  parent_span_id, "
        "  end_time - start_time AS computed_duration, duration "
        f"FROM view_instance('otel_spans', '{pid_str}') "
        "ORDER BY start_time"
    )
    rows = client.query(sql, begin, end)
    assert len(rows) >= 3
    rows = rows.head(3)
    names = list(rows["name"])
    assert names == ["root", "child-a", "child-b"], names
    kinds = list(rows["kind"])
    assert kinds == ["SERVER", "INTERNAL", "CLIENT"], kinds
    statuses = list(rows["status"])
    assert statuses == ["OK", "OK", "ERROR"], statuses
    # First span (root) has no parent.
    parent_ids = list(rows["parent_span_id"])
    assert parent_ids[0] is None or len(parent_ids[0]) == 0, parent_ids[0]
    # Children point at the root.
    assert bytes(parent_ids[1]) == root_span_id
    assert bytes(parent_ids[2]) == root_span_id
    # status_message non-null on the failing child.
    assert rows["status_message"].iloc[2] == "boom"
    # Duration column matches end_time - start_time.
    durations = list(rows["duration"])
    assert durations == [1_000_000, 400_000, 300_000], durations


# ---------------------------------------------------------------------------
# Idempotency
# ---------------------------------------------------------------------------


def test_otlp_idempotency_e2e():
    """POST the same logs payload twice — block_id is content-addressed, so
    the second insert hits ON CONFLICT (block_id) DO NOTHING and the row count
    stays at 5."""
    attrs, expected_pid = _fresh_resource_attrs()
    base_ns = _now_ns()
    req = _build_logs_request(attrs, base_ns)
    body = req.SerializeToString()

    for i in range(2):
        resp = requests.post(
            LOGS_ENDPOINT, data=body, headers=PROTOBUF_HEADERS, timeout=10
        )
        assert resp.status_code == 200, f"attempt {i}: {resp.text}"

    begin, end = _query_window()
    pid_str = str(expected_pid)

    def query_count():
        sql = (
            f"SELECT count(*) AS c FROM log_entries " f"WHERE process_id = '{pid_str}'"
        )
        return client.query(sql, begin, end)

    df = assert_eventually(
        query_count,
        lambda r: not r.empty and int(r.iloc[0]["c"]) >= 5,
        timeout_s=60,
        msg=f"waiting for 5 idempotent log_entries with process_id={pid_str}",
    )
    # Should be exactly 5 — the second POST is a no-op via block_id ON CONFLICT.
    assert int(df.iloc[0]["c"]) == 5, df.iloc[0]["c"]


# ---------------------------------------------------------------------------
# Content-Type rejection
# ---------------------------------------------------------------------------


def test_otlp_content_type_rejection():
    """OTLP/HTTP only accepts application/x-protobuf in v1; everything else
    must come back as 415 with a google.rpc.Status proto body."""
    # Use a syntactically valid empty logs request body, but advertise JSON.
    body = logs_service_pb2.ExportLogsServiceRequest().SerializeToString()
    resp = requests.post(
        LOGS_ENDPOINT,
        data=body,
        headers={"Content-Type": "application/json"},
        timeout=10,
    )
    assert resp.status_code == 415, resp.text
    assert resp.headers.get("content-type", "").startswith("application/x-protobuf")
    # The body should decode as a google.rpc.Status. The Rust hand-rolled
    # message has `code` (tag 1, int32) and `message` (tag 2, string). We
    # don't have the canonical google.rpc.Status proto on the Python side,
    # so decode as a generic protobuf and look at field 1.
    from opentelemetry.proto.common.v1 import common_pb2  # any small message

    # Use a tiny inline parser: read the first varint for `code`.
    raw = resp.content
    assert raw, "expected a Status proto body"
    # Field 1 (varint, wire-type 0) starts with tag = (1 << 3) | 0 = 8
    assert (
        raw[0] == 0x08
    ), f"expected tag 8 (field 1, varint) at start of Status, got {raw[0]:#x}"
    code = raw[1]  # small enough to fit in a single byte
    assert code == 3, f"expected INVALID_ARGUMENT(3), got {code}"
