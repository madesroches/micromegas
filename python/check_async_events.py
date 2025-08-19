import micromegas
import datetime

client = micromegas.connect()
now = datetime.datetime.now(datetime.timezone.utc)
begin = now - datetime.timedelta(hours=1)
end = now + datetime.timedelta(hours=1)

# Get the most recent telemetry-generator process
processes = client.query("SELECT process_id, exe, start_time FROM processes WHERE exe LIKE '%telemetry-generator%' ORDER BY start_time DESC LIMIT 1;", begin, end)
print('Most recent generator process:')
print(processes)

if len(processes) > 0:
    process_id = processes.iloc[0]['process_id']
    print(f'Checking async events for process: {process_id}')

    # Check async events with depth
    try:
        async_events = client.query(f"SELECT event_type, span_id, parent_span_id, depth, name FROM view_instance('async_events', '{process_id}') ORDER BY time LIMIT 10;", begin, end)
        print('Async events with depth:')
        print(async_events)
        print('Columns:', list(async_events.columns))

        if len(async_events) > 0:
            print(f'Found {len(async_events)} async events!')
            print('Depth values:', sorted(async_events['depth'].unique()))
        else:
            print('No async events found')
    except Exception as e:
        print(f'Error querying async events: {e}')
else:
    print('No generator process found')