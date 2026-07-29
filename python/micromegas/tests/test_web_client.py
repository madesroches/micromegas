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
