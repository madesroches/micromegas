#!/usr/bin/python3
import cbor2
import datetime
import io
import pyarrow.parquet as pq
import requests
import tabulate

ANALYTICS_BASE_URL = "http://localhost:8082/analytics/"


def request(url_tail, args):
    response = requests.post(
        ANALYTICS_BASE_URL + url_tail,
        data=cbor2.dumps(args),
    )
    if response.status_code != 200:
        raise Exception(
            "http request failed code={0} text={1}".format(
                response.status_code, response.text
            )
        )
    table = pq.read_table(io.BytesIO(response.content))
    return table.to_pandas()


def req(url_tail, args={}):
    # add default args that make sense for tests but would not in general
    if "begin" not in args:
        # set a very large time span if there is not already one specified
        end = datetime.datetime.now(datetime.timezone.utc)
        begin = end - datetime.timedelta(days=10000)
        end = end + datetime.timedelta(hours=1)
        args["begin"] = begin.isoformat()
        args["end"] = end.isoformat()
    if "limit" not in args:
        args["limit"] = 1024
    return request(url_tail, args)


def test_process_list():
    df = req("query_processes")
    df = df[["process_id", "exe", "username", "start_time", "insert_time"]]
    print(tabulate.tabulate(df, headers="keys"))


def test_list_streams():
    df = req("query_streams")
    print(df)


def test_find_cpu_stream():
    df = req("query_streams", args={"tag_filter": "cpu"})
    print(df)
