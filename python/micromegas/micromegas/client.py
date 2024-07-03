from . import request
from . import time
            

class Client:
    def __init__(self, base_url, headers={}):
        self.analytics_base_url = base_url + "analytics/"
        self.headers = headers

    def find_process(self, process_id):
        return request.request(
            self.analytics_base_url + "find_process",
            {"process_id": process_id},
            headers=self.headers,
        )

    def query_processes(self, begin, end, limit):
        return request.request(
            self.analytics_base_url + "query_processes",
            {"begin": time.format_datetime(begin), "end": time.format_datetime(end), "limit": limit},
            headers=self.headers,
        )

    def query_streams(self, begin, end, limit, process_id=None, tag_filter=None):
        args = {
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
            "limit": limit,
            "process_id": process_id,
            "tag_filter": tag_filter,
        }

        return request.request(
            self.analytics_base_url + "query_streams",
            args,
            headers=self.headers,
        )

    def query_blocks(self, begin, end, limit, stream_id):
        args = {
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
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
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
            },
            headers=self.headers,
        )

    def query_thread_events(self, begin, end, limit, stream_id):
        return request.request(
            self.analytics_base_url + "query_thread_events",
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
            },
            headers=self.headers,
        )

    def query_log_entries(self, begin, end, limit, stream_id):
        return request.request(
            self.analytics_base_url + "query_log_entries",
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
            },
            headers=self.headers,
        )

    def query_metrics(self, begin, end, limit, stream_id):
        return request.request(
            self.analytics_base_url + "query_metrics",
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
            },
            headers=self.headers,
        )
