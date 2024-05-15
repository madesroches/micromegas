from . import request


class Client:
    def __init__(self, base_url, headers={}):
        self.analytics_base_url = base_url + "analytics/"
        self.headers = headers

    def query_processes(self, begin, end, limit):
        return request.request(
            self.analytics_base_url + "query_processes",
            {"begin": begin.isoformat(), "end": end.isoformat(), "limit": limit},
            headers=self.headers,
        )

    def query_streams(self, begin, end, limit, tag_filter=None):
        args = {
            "begin": begin.isoformat(),
            "end": end.isoformat(),
            "limit": limit,
        }

        if tag_filter is not None:
            args["tag_filter"] = tag_filter

        return request.request(
            self.analytics_base_url + "query_streams",
            args,
            headers=self.headers,
        )

    def query_blocks(self, begin, end, limit, stream_id):
        args = {
            "begin": begin.isoformat(),
            "end": end.isoformat(),
            "limit": limit,
            "stream_id": stream_id,
        }

        return request.request(
            self.analytics_base_url + "query_blocks",
            args,
            headers=self.headers,
        )

    def query_spans(self, begin, end, limit, stream_id):
        return request.request(
            self.analytics_base_url + "query_spans",
            {
                "begin": begin.isoformat(),
                "end": end.isoformat(),
                "limit": limit,
                "stream_id": stream_id,
            },
            headers=self.headers,
        )
