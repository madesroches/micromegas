import cbor2
import io
import pyarrow.parquet as pq
import requests


def request(url, args, headers={}):
    response = requests.post(
        url,
        headers=headers,
        data=cbor2.dumps(args),
    )
    if response.status_code != 200:
        raise Exception(
            "http request url={2} failed with code={0} text={1}".format(
                response.status_code, response.text, url
            )
        )
    table = pq.read_table(io.BytesIO(response.content))
    return table.to_pandas()

def streamed_request(url, args, headers={}):
    response = requests.post(
        url,
        headers=headers,
        data=cbor2.dumps(args),
        stream=True,
        timeout=300,
    )
    if response.status_code != 200:
        raise Exception(
            "http request url={2} failed with code={0} text={1}".format(
                response.status_code, response.text, url
            )
        )
    return response
