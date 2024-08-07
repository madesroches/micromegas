from . import request
from . import time
import cbor2


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
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
            },
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

    def query_log_entries(
        self,
        begin,
        end,
        limit=None,  # Necessary if stream_id is specified, ignored otherwise
        stream_id=None,  # If none, query is run on cached lakehouse using query engine
        sql=None,  # Necessary if stream_id is None, ignored otherwise
    ):
        return request.request(
            self.analytics_base_url + "query_log_entries",
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
                "sql": sql,
            },
            headers=self.headers,
        )

    def query_metrics(self, begin, end, limit=None, stream_id=None, sql=None):
        return request.request(
            self.analytics_base_url + "query_metrics",
            {
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "limit": limit,
                "stream_id": stream_id,
                "sql": sql,
            },
            headers=self.headers,
        )

    def query_view(self, view_set_name, view_instance_id, begin, end, sql):
        return request.request(
            self.analytics_base_url + "query_view",
            {
                "view_set_name": view_set_name,
                "view_instance_id": view_instance_id,
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
                "sql": sql,
            },
            headers=self.headers,
        )

    def query_partitions(self):
        args = {}
        return request.request(
            self.analytics_base_url + "query_partitions",
            args,
            headers=self.headers,
        )

    def __stream_request(self, endpoint, args):
        response = request.streamed_request(
            self.analytics_base_url + endpoint,
            args,
            headers=self.headers,
        )
        while response.raw.readable():
            try:
                print(cbor2.load(response.raw))
            except cbor2.CBORDecodeEOF:
                break

    def create_or_update_partitions(
        self, view_set_name, view_instance_id, begin, end, partition_delta_seconds
    ):
        args = {
            "view_set_name": view_set_name,
            "view_instance_id": view_instance_id,
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
            "partition_delta_seconds": partition_delta_seconds,
        }
        self.__stream_request("create_or_update_partitions", args)

    def merge_partitions(
        self, view_set_name, view_instance_id, begin, end, partition_delta_seconds
    ):
        args = {
            "view_set_name": view_set_name,
            "view_instance_id": view_instance_id,
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
            "partition_delta_seconds": partition_delta_seconds,
        }
        self.__stream_request("merge_partitions", args)

    def retire_partitions(
        self, view_set_name, view_instance_id, begin, end, partition_delta_seconds
    ):
        args = {
            "view_set_name": view_set_name,
            "view_instance_id": view_instance_id,
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
            "partition_delta_seconds": partition_delta_seconds,
        }
        self.__stream_request("retire_partitions", args)
