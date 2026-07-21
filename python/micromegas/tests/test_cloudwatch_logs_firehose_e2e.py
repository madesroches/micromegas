"""End-to-end test of the CloudWatch Logs subscription-filter Firehose ingestion path.

Kept separate from `test_otlp_e2e.py`: this route carries no OTLP payload on the
wire (see `tasks/1300_cloudwatch_logs_firehose_plan.md`), only CloudWatch's own
proprietary gzip+JSON subscription-filter format, so none of the OTLP proto test
helpers apply here.

Builds a gzip-compressed CloudWatch Logs `DATA_MESSAGE` (and a `CONTROL_MESSAGE`)
by hand, wraps each in a Firehose envelope, and POSTs to
`/ingestion/cloudwatch/v1/logs/firehose`. Assumes services are already running:
    python3 local_test_env/ai_scripts/start_services.py
"""

import base64
import datetime
import gzip
import json
import os
import time
import uuid

import requests

from .otlp_helpers import assert_eventually, discover_process_id_by_property
from .test_utils import client


INGESTION_URL = os.environ.get("MICROMEGAS_INGESTION_URL", "http://127.0.0.1:9000")
FIREHOSE_ENDPOINT = f"{INGESTION_URL}/ingestion/cloudwatch/v1/logs/firehose"

# CloudWatch Logs data materializes into the global views within a second or two
# (the maintenance daemon's per-second task). Poll with a small margin over that.
POLL_TIMEOUT_S = 15


def _query_window():
    """A wide [begin, end] window centered on now — covers JIT + clock skew."""
    now = datetime.datetime.now(datetime.timezone.utc)
    return now - datetime.timedelta(hours=1), now + datetime.timedelta(hours=1)


def _gzip(data: bytes) -> bytes:
    return gzip.compress(data)


def _firehose_envelope(request_id, gzipped_records):
    """Wrap a list of gzip-compressed CloudWatch Logs JSON records in a Firehose
    JSON envelope (base64-encoded `data` per record)."""
    return json.dumps(
        {
            "requestId": request_id,
            "timestamp": int(time.time() * 1000),
            "records": [
                {"data": base64.b64encode(r).decode("ascii")} for r in gzipped_records
            ],
        }
    )


def _data_message(log_group, log_stream, owner, log_events):
    payload = {
        "messageType": "DATA_MESSAGE",
        "owner": owner,
        "logGroup": log_group,
        "logStream": log_stream,
        "subscriptionFilters": ["e2e-filter"],
        "logEvents": log_events,
    }
    return _gzip(json.dumps(payload).encode("utf-8"))


def _control_message(event_id):
    # CONTROL_MESSAGE payloads always carry empty logGroup/logStream/a fixed owner per
    # the CloudWatch spec, so no per-test-run identifier can go in those fields — the
    # synthetic health-check logEvent's `id` is the only field this test can make unique,
    # letting the negative assertion below anchor on something actually present in this
    # specific request rather than a value that was never sent.
    payload = {
        "messageType": "CONTROL_MESSAGE",
        "owner": "CloudwatchLogs",
        "logGroup": "",
        "logStream": "",
        "subscriptionFilters": [],
        "logEvents": [
            {
                "id": event_id,
                "timestamp": int(time.time() * 1000),
                "message": "CWL CONTROL MESSAGE: Checking health of destination Firehose.",
            }
        ],
    }
    return _gzip(json.dumps(payload).encode("utf-8"))


def test_cloudwatch_logs_firehose_e2e():
    """POST a CloudWatch Logs DATA_MESSAGE (2 logEvents) wrapped in a Firehose
    envelope; assert the ack echoes the request id and the events land in
    `log_entries` with the expected msg/time and process_properties carrying the
    log group/stream."""
    log_stream = f"e2e-stream-{uuid.uuid4()}"
    log_group = "/ecs/cloudwatch-logs-e2e"
    owner = "123456789012"
    now_ms = int(time.time() * 1000)
    log_events = [
        {"id": "evt-1", "timestamp": now_ms, "message": "e2e log line one"},
        {"id": "evt-2", "timestamp": now_ms + 1, "message": "e2e log line two"},
    ]
    record = _data_message(log_group, log_stream, owner, log_events)
    request_id = f"cw-logs-e2e-{uuid.uuid4()}"
    body = _firehose_envelope(request_id, [record])

    resp = requests.post(
        FIREHOSE_ENDPOINT,
        data=body,
        headers={
            "Content-Type": "application/json",
            "X-Amz-Firehose-Request-Id": request_id,
        },
        timeout=10,
    )
    assert resp.status_code == 200, resp.text
    ack = resp.json()
    assert ack["requestId"] == request_id
    assert "errorMessage" not in ack

    begin, end = _query_window()
    pid_str = discover_process_id_by_property(
        client,
        "otel.resource.aws.log.stream.name",
        log_stream,
        begin,
        end,
        timeout_s=POLL_TIMEOUT_S,
    )

    def query_rows():
        sql = (
            "SELECT msg, time FROM log_entries "
            f"WHERE process_id = '{pid_str}' ORDER BY time"
        )
        return client.query(sql, begin, end)

    df = assert_eventually(
        query_rows,
        lambda r: not r.empty and len(r) >= 2,
        timeout_s=POLL_TIMEOUT_S,
        msg=f"waiting for 2 log_entries rows with process_id={pid_str}",
    )
    assert len(df) >= 2
    msgs = list(df["msg"][:2])
    assert msgs == ["e2e log line one", "e2e log line two"], msgs

    sql = (
        "SELECT jsonb_as_string(jsonb_get(process_properties, "
        "'otel.resource.aws.log.group.name')) AS log_group, "
        "  jsonb_as_string(jsonb_get(process_properties, "
        "'otel.resource.aws.log.stream.name')) AS log_stream "
        f"FROM log_entries WHERE process_id = '{pid_str}' LIMIT 1"
    )
    props = client.query(sql, begin, end)
    assert props.iloc[0]["log_group"] == log_group
    assert props.iloc[0]["log_stream"] == log_stream


def test_cloudwatch_logs_firehose_control_message_is_ignored():
    """A CONTROL_MESSAGE record must ack 200 but add no log_entries row.

    The negative assertion is anchored on a per-run-unique CloudWatch event id
    embedded in the (otherwise entirely fixed/empty) CONTROL_MESSAGE payload: if
    CONTROL_MESSAGE handling ever regressed and the message were processed like a
    DATA_MESSAGE, the resulting row's `properties` would carry this exact
    `aws.log.event.id`, so querying for it is a real, run-specific check — unlike
    querying by a value that was never part of the payload."""
    event_id = f"e2e-control-{uuid.uuid4()}"
    request_id = f"cw-logs-control-e2e-{uuid.uuid4()}"
    body = _firehose_envelope(request_id, [_control_message(event_id)])

    resp = requests.post(
        FIREHOSE_ENDPOINT,
        data=body,
        headers={
            "Content-Type": "application/json",
            "X-Amz-Firehose-Request-Id": request_id,
        },
        timeout=10,
    )
    assert resp.status_code == 200, resp.text
    ack = resp.json()
    assert ack["requestId"] == request_id
    assert "errorMessage" not in ack

    # No process/row is ever registered for a control message (it never reaches
    # write_blocks), so there's nothing to poll for landing — a short grace sleep
    # plus a negative lookup on this run's unique event id is the only way to
    # assert absence.
    time.sleep(2)
    begin, end = _query_window()
    sql = (
        "SELECT count(*) AS c FROM log_entries "
        f"WHERE jsonb_as_string(jsonb_get(properties, 'aws.log.event.id')) = '{event_id}'"
    )
    df = client.query(sql, begin, end)
    assert int(df.iloc[0]["c"]) == 0


def test_cloudwatch_logs_firehose_dev_mode_open_without_access_key():
    """The local test stack runs ingestion with --disable-auth, so (like every
    other ingestion route) this route accepts requests with no
    X-Amz-Firehose-Access-Key header at all."""
    request_id = f"cw-logs-dev-mode-{uuid.uuid4()}"
    body = _firehose_envelope(request_id, [])

    resp = requests.post(
        FIREHOSE_ENDPOINT,
        data=body,
        headers={
            "Content-Type": "application/json",
            "X-Amz-Firehose-Request-Id": request_id,
        },
        timeout=10,
    )
    assert resp.status_code == 200, resp.text
    assert resp.json()["requestId"] == request_id
