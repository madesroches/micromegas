#!/usr/bin/env python3
"""Launch Claude Code with the OTel resource attributes its built-in detector
does not emit.

Without these, our `process_id_from_resource` formula collapses every Claude
session onto one process_id (host.id, host.name, process.pid, and
service.instance.id are all empty in Claude's default Resource).

Re-running this script produces a fresh service.instance.id per invocation —
putting the same export in a shell rc file would not, because $(uuidgen)
would be evaluated once at shell startup.

Usage:
    python3 local_test_env/claude_code_otel.py [claude-args...]

Honors the same env vars `telemetry-sink` (the native producer) reads:
    MICROMEGAS_TELEMETRY_URL       base URL of telemetry-ingestion-srv
                                   (default http://localhost:9000) — we append
                                   /ingestion/otlp so the OTel SDK's appended
                                   /v1/{logs,metrics,traces} lands on the
                                   right routes.
    MICROMEGAS_INGESTION_API_KEY   optional bearer token (matches an entry in
                                   MICROMEGAS_API_KEYS on the server).

Plus:
    OTEL_RESOURCE_ATTRIBUTES       caller-set attrs are preserved and appended
                                   after the identity tuple this script injects.

Tracing (beta) is enabled so spans land in the `otel_spans` view via the same
OTLP endpoint. Claude Code's traces are gated behind
`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1`; we set it here.

Verbose telemetry:
    Pass --verbose-telemetry (or set MICROMEGAS_OTEL_VERBOSE=1) to include
    user prompts, tool input details, full Anthropic Messages API request and
    response bodies, and tool input/output content on span events. This sets:
        OTEL_LOG_USER_PROMPTS=1     prompt text on user_prompt events
        OTEL_LOG_TOOL_DETAILS=1     tool input args on tool_result events
        OTEL_LOG_RAW_API_BODIES=1   api_request_body / api_response_body events
        OTEL_LOG_TOOL_CONTENT=1     tool input/output on span events (traces)
    Bodies and span content are truncated at 60 KB by Claude Code; raw bodies
    include the entire conversation history.
    The flag is stripped from argv before invoking claude.

Works on Linux, macOS, and Windows (PowerShell or cmd).
"""

import os
import shutil
import socket
import subprocess
import sys
import uuid


VERBOSE_FLAG = "--verbose-telemetry"


def _truthy(value: str) -> bool:
    return value.strip().lower() in ("1", "true", "yes", "on")


def main() -> int:
    args = sys.argv[1:]
    verbose = _truthy(os.environ.get("MICROMEGAS_OTEL_VERBOSE", ""))
    if VERBOSE_FLAG in args:
        verbose = True
        args = [a for a in args if a != VERBOSE_FLAG]

    base_url = os.environ.get("MICROMEGAS_TELEMETRY_URL", "http://localhost:9000").rstrip("/")
    api_key = os.environ.get("MICROMEGAS_INGESTION_API_KEY", "")

    identity_attrs = f"service.instance.id={uuid.uuid4()},host.name={socket.gethostname()}"
    existing_attrs = os.environ.get("OTEL_RESOURCE_ATTRIBUTES", "")
    resource_attrs = f"{identity_attrs},{existing_attrs}" if existing_attrs else identity_attrs

    env = os.environ.copy()
    env["OTEL_RESOURCE_ATTRIBUTES"] = resource_attrs
    env["CLAUDE_CODE_ENABLE_TELEMETRY"] = "1"
    env["OTEL_EXPORTER_OTLP_ENDPOINT"] = f"{base_url}/ingestion/otlp"
    env["OTEL_EXPORTER_OTLP_PROTOCOL"] = "http/protobuf"
    env["OTEL_METRICS_EXPORTER"] = "otlp"
    env["OTEL_LOGS_EXPORTER"] = "otlp"
    env["OTEL_TRACES_EXPORTER"] = "otlp"
    env["CLAUDE_CODE_ENHANCED_TELEMETRY_BETA"] = "1"
    if api_key:
        env["OTEL_EXPORTER_OTLP_HEADERS"] = f"Authorization=Bearer {api_key}"

    if verbose:
        env["OTEL_LOG_USER_PROMPTS"] = "1"
        env["OTEL_LOG_TOOL_DETAILS"] = "1"
        env["OTEL_LOG_RAW_API_BODIES"] = "1"
        env["OTEL_LOG_TOOL_CONTENT"] = "1"

    claude = shutil.which("claude") or shutil.which("claude.cmd") or shutil.which("claude.exe")
    if claude is None:
        print("claude_code_otel.py: 'claude' not found on PATH", file=sys.stderr)
        return 127

    return subprocess.run([claude, *args], env=env).returncode


if __name__ == "__main__":
    sys.exit(main())
