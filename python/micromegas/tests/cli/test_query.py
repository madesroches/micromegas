import sys

import pandas as pd

import micromegas.cli.query as query_module


class _FakeClient:
    def query(self, sql, begin=None, end=None):
        return pd.DataFrame()


def test_main_passes_cli_query_entrypoint_to_connect(monkeypatch, capsys):
    captured_kwargs = {}

    def fake_connect(**kwargs):
        captured_kwargs.update(kwargs)
        return _FakeClient()

    monkeypatch.setattr(query_module.connection, "connect", fake_connect)
    monkeypatch.setattr(sys, "argv", ["micromegas-query", "SELECT 1", "--all"])

    query_module.main()

    assert captured_kwargs.get("client_entrypoint") == "cli-query"
    out = capsys.readouterr().out
    assert "no data" in out
