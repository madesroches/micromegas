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


class TestMintIngestionApiKey:
    """`mint_ingestion_api_key` (AbAC Stage 6, #1374): payload construction and the
    409 `CLAIM_CONTENDED` retry-once logic."""

    def test_omits_audience_when_none(self):
        client = _make_client()
        client.session.post.return_value.json.return_value = {"key": "mmk_x"}
        client.mint_ingestion_api_key("laptop")
        payload = client.session.post.call_args.kwargs["json"]
        assert payload == {"name": "laptop"}

    def test_includes_audience_when_set(self):
        client = _make_client()
        client.session.post.return_value.json.return_value = {"key": "mmk_x"}
        client.mint_ingestion_api_key("laptop", audience="team-alpha")
        payload = client.session.post.call_args.kwargs["json"]
        assert payload == {"name": "laptop", "audience": "team-alpha"}

    def test_posts_to_ingestion_api_keys(self):
        client = _make_client()
        client.session.post.return_value.json.return_value = {"key": "mmk_x"}
        client.mint_ingestion_api_key("laptop")
        call = client.session.post.call_args
        assert call.args[0] == "http://localhost:9999/api/ingestion-api-keys"

    def test_success_on_first_try_returns_body_with_no_retry(self):
        client = _make_client()
        resp = MagicMock()
        resp.status_code = 201
        resp.ok = True
        resp.json.return_value = {"key": "mmk_x", "key_id": "abc"}
        client.session.post.return_value = resp

        result = client.mint_ingestion_api_key("laptop", audience="fresh")

        assert result == {"key": "mmk_x", "key_id": "abc"}
        assert client.session.post.call_count == 1

    def test_retries_once_on_claim_contended_then_succeeds(self):
        client = _make_client()
        contended = MagicMock()
        contended.status_code = 409
        contended.ok = False
        contended.json.return_value = {"code": "CLAIM_CONTENDED", "message": "retry"}

        success = MagicMock()
        success.status_code = 201
        success.ok = True
        success.json.return_value = {"key": "mmk_x", "key_id": "abc"}

        client.session.post.side_effect = [contended, success]

        result = client.mint_ingestion_api_key("laptop", audience="fresh")

        assert result == {"key": "mmk_x", "key_id": "abc"}
        assert client.session.post.call_count == 2

    def test_retries_at_most_once_a_second_claim_contended_raises(self):
        client = _make_client()
        contended = MagicMock()
        contended.status_code = 409
        contended.ok = False
        contended.json.return_value = {"code": "CLAIM_CONTENDED", "message": "retry"}
        contended.text = "conflict"
        client.session.post.side_effect = [contended, contended]

        try:
            client.mint_ingestion_api_key("laptop", audience="fresh")
            assert False, "expected RuntimeError"
        except RuntimeError as e:
            assert "409" in str(e)
        assert client.session.post.call_count == 2

    def test_non_claim_contended_409_is_not_retried(self):
        client = _make_client()
        resp = MagicMock()
        resp.status_code = 409
        resp.ok = False
        resp.json.return_value = {"code": "SOME_OTHER_CODE", "message": "nope"}
        resp.text = "nope"
        client.session.post.return_value = resp

        try:
            client.mint_ingestion_api_key("laptop", audience="fresh")
            assert False, "expected RuntimeError"
        except RuntimeError:
            pass
        assert client.session.post.call_count == 1

    def test_403_forbidden_is_not_retried(self):
        client = _make_client()
        resp = MagicMock()
        resp.status_code = 403
        resp.ok = False
        resp.json.return_value = {"code": "FORBIDDEN", "message": "denied"}
        resp.text = "denied"
        client.session.post.return_value = resp

        try:
            client.mint_ingestion_api_key("laptop", audience="fresh")
            assert False, "expected RuntimeError"
        except RuntimeError:
            pass
        assert client.session.post.call_count == 1


class TestMyAudiences:
    """`my_audiences` (AbAC Stage 6, #1374): `GET .../audience-grants/my-audiences`."""

    def test_calls_the_my_audiences_route(self):
        client = _make_client()
        client.session.get.return_value.json.return_value = {
            "is_admin": False,
            "audiences": ["team-alpha"],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
        }
        result = client.my_audiences()
        call = client.session.get.call_args
        assert call.args[0] == "http://localhost:9999/api/audience-grants/my-audiences"
        assert result == {
            "is_admin": False,
            "audiences": ["team-alpha"],
            "mint_prefix": "alice-",
            "email": "alice@example.com",
        }
