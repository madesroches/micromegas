from .test_utils import *

def test_log_query():
    print("searching for stream")
    stream_id = get_tagged_stream_with_most_events("log")
    # print("log stream", stream_id)
    # log_entries = client.query_log_entries(begin, end, limit, stream_id)
    # print(log_entries)
    stream = client.find_stream(stream_id)
    print(stream)
    #todo: get process associated with stream
    #todo: lh query this process's log
