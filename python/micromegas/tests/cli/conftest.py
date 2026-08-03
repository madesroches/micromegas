import os

import pytest


@pytest.fixture(autouse=True)
def scrub_micromegas_env(monkeypatch):
    """Scrub every MICROMEGAS_* env var before each test.

    Without this, a developer's exported MICROMEGAS_PROFILE (or any other
    MICROMEGAS_* var) leaks into resolve_connection/resolve_active_profile
    and can break the flat-config tests (e.g. by routing them into the
    "no profiles configured" ProfileError path).
    """
    for key in list(os.environ):
        if key.startswith("MICROMEGAS_"):
            monkeypatch.delenv(key)
