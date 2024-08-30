import datetime
import pandas as pd
import micromegas

BASE_URL = "http://localhost:8082/"
client = micromegas.client.Client(BASE_URL)

now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(days=10000)
end = now + datetime.timedelta(hours=1)
limit = 1024

def get_tagged_streams_with_data(tag_filter):
    streams_df = client.query_streams(begin, end, limit, tag_filter=tag_filter)
    streams_stats = {}
    for index, row in streams_df.iterrows():
        blocks_df = client.query_blocks(begin, end, limit, row["stream_id"])
        if len(blocks_df) == 0:
            stats = {"sum_payload": 0, "nb_events": 0}
        else:
            stats = {
                "sum_payload": blocks_df["payload_size"].sum(),
                "nb_events": blocks_df["nb_objects"].sum(),
            }
        streams_stats[row["stream_id"]] = stats
    streams_stats = pd.DataFrame(streams_stats).transpose()
    streams_stats = streams_stats[streams_stats["nb_events"] > 0]
    return streams_stats

def get_tagged_stream_with_most_events(tag_filter):
    streams_stats = get_tagged_streams_with_data(tag_filter)
    streams_stats = streams_stats.sort_values("nb_events", ascending=False)
    #print(streams_stats)
    return streams_stats.iloc[0].name
