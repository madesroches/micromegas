#!/bin/python3
import os
from rust_command import run_command

os.environ["PUBLISH_GRACE_SLEEP"] = "60"

run_command("cargo fmt --check")
run_command("cargo release -p micromegas-derive-transit -x --no-confirm")
run_command("cargo release -p micromegas-transit -x --no-confirm")
run_command("cargo release -p micromegas-tracing-proc-macros -x --no-confirm")
run_command("cargo release -p micromegas-tracing -x --no-confirm")
run_command("cargo release -p micromegas-telemetry -x --no-confirm")
run_command("cargo release -p micromegas-ingestion -x --no-confirm")
run_command("cargo release -p micromegas-telemetry-sink -x --no-confirm")
run_command("cargo release -p micromegas-analytics -x --no-confirm")
run_command("cargo release -p micromegas-perfetto -x --no-confirm")
run_command("cargo release -p micromegas -x --no-confirm")
