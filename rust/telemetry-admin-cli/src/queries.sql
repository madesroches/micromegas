-- process by size
SELECT processes.process_id, format_bytes(SUM(blocks.payload_size)) as SIZE
FROM   processes, streams, blocks
WHERE  streams.process_id = processes.process_id
AND    blocks.stream_id = streams.stream_id
GROUP BY processes.process_id
ORDER BY SUM(blocks.payload_size)
;

-- size by date
SELECT DATE(processes.start_time) as START_DATE, format_bytes(SUM(blocks.payload_size)) as SIZE
FROM   processes, streams, blocks
WHERE  streams.process_id = processes.process_id
AND    blocks.stream_id = streams.stream_id
GROUP BY START_DATE
ORDER BY START_DATE
;

-- size by stream type
SELECT streams.tags as TAGS, format_bytes(SUM(blocks.payload_size)) as SIZE
FROM   streams, blocks
WHERE  blocks.stream_id = streams.stream_id
GROUP BY streams.tags
ORDER BY streams.tags
;

-- size by datediff
SELECT DATEDIFF(NOW(), processes.start_time) as DIFF, format_bytes(SUM(blocks.payload_size)) as SIZE
FROM   processes, streams, blocks
WHERE  streams.process_id = processes.process_id
AND    blocks.stream_id = streams.stream_id
GROUP BY DIFF
ORDER BY DIFF
;

-- blocks 29 days or older
SELECT DATEDIFF(NOW(), processes.start_time) as DIFF, blocks.block_id, payloads.block_id
FROM   processes, streams, blocks
LEFT JOIN payloads ON blocks.block_id = payloads.block_id
WHERE  streams.process_id = processes.process_id
AND    blocks.stream_id = streams.stream_id
AND    DATEDIFF(NOW(), processes.start_time) >= 29
;
