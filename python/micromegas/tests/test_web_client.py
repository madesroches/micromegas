"""Unit tests for WebClient's create_screen/update_screen payload construction."""

from unittest.mock import MagicMock

from micromegas.web_client import WebClient


def _make_client():
    client = WebClient("http://localhost:9999")
    client.session = MagicMock()
    client.session.post.return_value.ok = True
    client.session.post.return_value.json.return_value = {}
    client.session.put.return_value.ok = True
    client.session.put.return_value.json.return_value = {}
    return client


class TestCreateScreenFolderPath:
    def test_omitted_when_none(self):
        client = _make_client()
        client.create_screen("s", "notebook", {})
        payload = client.session.post.call_args.kwargs["json"]
        assert "folder_path" not in payload

    def test_included_when_empty_string(self):
        client = _make_client()
        client.create_screen("s", "notebook", {}, folder_path="")
        payload = client.session.post.call_args.kwargs["json"]
        assert payload["folder_path"] == ""

    def test_included_when_set(self):
        client = _make_client()
        client.create_screen("s", "notebook", {}, folder_path="dashboards/team-a")
        payload = client.session.post.call_args.kwargs["json"]
        assert payload["folder_path"] == "dashboards/team-a"


class TestUpdateScreenFolderPath:
    def test_omitted_when_none(self):
        client = _make_client()
        client.update_screen("s", {})
        payload = client.session.put.call_args.kwargs["json"]
        assert "folder_path" not in payload

    def test_included_when_empty_string(self):
        client = _make_client()
        client.update_screen("s", {}, folder_path="")
        payload = client.session.put.call_args.kwargs["json"]
        assert payload["folder_path"] == ""

    def test_included_when_set(self):
        client = _make_client()
        client.update_screen("s", {}, folder_path="dashboards/team-a")
        payload = client.session.put.call_args.kwargs["json"]
        assert payload["folder_path"] == "dashboards/team-a"


class TestImportIngestionApiKeyAudience:
    """`audience` (#1372, AbAC Stage 4) follows the same omitted/set convention
    as `folder_path` above -- but no empty-string case: unlike `folder_path`,
    an empty-string audience is never a meaningful value to transmit (the
    server's `resolve_audience` treats it as absent either way), so this
    class only pins the two cases that differ in behavior."""

    def test_omitted_when_none(self):
        client = _make_client()
        client.import_ingestion_api_key("k", "secret")
        payload = client.session.post.call_args.kwargs["json"]
        assert "audience" not in payload

    def test_included_when_set(self):
        client = _make_client()
        client.import_ingestion_api_key("k", "secret", audience="team-alpha")
        payload = client.session.post.call_args.kwargs["json"]
        assert payload["audience"] == "team-alpha"
