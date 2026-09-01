"""Re-export of the top-level profile-aware connect, kept for `cli/query.py`
and any other existing `cli.connection.connect(...)` call sites.

The implementation lives in `micromegas.connection` so it's reachable as
`micromegas.connect_with_profile()` without importing a `cli` submodule.
"""

from micromegas.connection import connect_with_profile as connect
