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

    def find_stream(self, stream_id):
        return request.request(
            self.analytics_base_url + "find_stream",
            {"stream_id": stream_id},
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

    def query(self, sql, begin=None, end=None):
        return request.request(
            self.analytics_base_url + "query",
            {
                "sql": sql,
                "begin": time.format_datetime(begin),
                "end": time.format_datetime(end),
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

    def materialize_partitions(
        self, view_set_name, view_instance_id, begin, end, partition_delta_seconds
    ):
        args = {
            "view_set_name": view_set_name,
            "view_instance_id": view_instance_id,
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
            "partition_delta_seconds": partition_delta_seconds,
        }
        self.__stream_request("materialize_partitions", args)

    def retire_partitions(self, view_set_name, view_instance_id, begin, end):
        args = {
            "view_set_name": view_set_name,
            "view_instance_id": view_instance_id,
            "begin": time.format_datetime(begin),
            "end": time.format_datetime(end),
        }
        self.__stream_request("retire_partitions", args)
