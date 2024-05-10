import cbor2
import io
import pyarrow.parquet as pq
import requests

def request(url, args):
    response = requests.post(
        url,
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
