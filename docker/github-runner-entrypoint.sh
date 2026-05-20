#!/usr/bin/env bash
set -euo pipefail

# Read the registration token from the bind-mounted secret file.
TOKEN=$(cat /run/secrets/registration-token)

# Configure the runner (ephemeral, labeled, non-interactive).
./config.sh \
  --url "https://github.com/${REPO}" \
  --token "${TOKEN}" \
  --name "${RUNNER_NAME}" \
  --labels "dev-worker,linux,${ARCH}" \
  --work _work \
  --ephemeral \
  --unattended \
  --replace

# Run the runner agent. It picks up one job, executes it, then exits.
# exec replaces the shell so signals are forwarded directly to the runner.
exec ./run.sh
