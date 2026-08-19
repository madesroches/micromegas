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
    client.session.get.return_value.ok = True
    client.session.get.return_value.json.return_value = []
    client.session.delete.return_value.ok = True
    client.session.delete.return_value.json.return_value = {}
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


class TestAudienceGrants:
    """#1489, AbAC Stage 6a: `create_audience_grant`/`list_audience_grants`/
    `delete_audience_grant` payload/params construction."""

    def test_create_audience_grant_payload(self):
        client = _make_client()
        client.create_audience_grant("team-alpha", "read", "group:eng")
        call = client.session.post.call_args
        assert call.args[0] == "http://localhost:9999/api/audience-grants"
        assert call.kwargs["json"] == {
            "audience": "team-alpha",
            "axis": "read",
            "selector": "group:eng",
        }

    def test_list_audience_grants_omits_unset_filters(self):
        client = _make_client()
        client.list_audience_grants()
        call = client.session.get.call_args
        assert call.args[0] == "http://localhost:9999/api/audience-grants"
        assert call.kwargs["params"] == {}

    def test_list_audience_grants_includes_set_filters(self):
        client = _make_client()
        client.list_audience_grants(
            audience="team-alpha", axis="mint", limit=10, offset=5
        )
        call = client.session.get.call_args
        assert call.kwargs["params"] == {
            "audience": "team-alpha",
            "axis": "mint",
            "limit": 10,
            "offset": 5,
        }

    def test_delete_audience_grant_passes_natural_key_as_query_params(self):
        client = _make_client()
        client.delete_audience_grant("team-alpha", "read", "user:alice@example.com")
        call = client.session.delete.call_args
        assert call.args[0] == "http://localhost:9999/api/audience-grants"
        assert call.kwargs["params"] == {
            "audience": "team-alpha",
            "axis": "read",
            "selector": "user:alice@example.com",
        }
