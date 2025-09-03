#!/bin/python3
import os
from rust_command import run_command

# Set grace period between publishes to allow crates.io to index
os.environ["PUBLISH_GRACE_SLEEP"] = "60"

# Format check before starting
run_command("cargo fmt --check")

# Release crates in dependency order
# Layer 1: Foundation crates (no internal dependencies)
run_command("cargo release -p micromegas-derive-transit -x --no-confirm")
run_command("cargo release -p micromegas-tracing-proc-macros -x --no-confirm")

# Layer 2: Core serialization (depends on derive-transit)
run_command("cargo release -p micromegas-transit -x --no-confirm")

# Layer 3: Tracing (depends on transit, proc-macros)  
run_command("cargo release -p micromegas-tracing -x --no-confirm")

# Layer 4: Telemetry data structures (depends on tracing, transit)
run_command("cargo release -p micromegas-telemetry -x --no-confirm")

# Layer 5: Core services (depend on telemetry, tracing, transit)
run_command("cargo release -p micromegas-ingestion -x --no-confirm")
run_command("cargo release -p micromegas-telemetry-sink -x --no-confirm")

# Layer 6: Perfetto (depends on tracing, transit)
run_command("cargo release -p micromegas-perfetto -x --no-confirm")

# Layer 7: Analytics (depends on ingestion, telemetry, tracing, transit, perfetto)
run_command("cargo release -p micromegas-analytics -x --no-confirm")

# Layer 8: Top-level proc macros (depends on tracing, analytics)
run_command("cargo release -p micromegas-proc-macros -x --no-confirm")

# Layer 9: Main public crate (depends on all others)
run_command("cargo release -p micromegas -x --no-confirm")
