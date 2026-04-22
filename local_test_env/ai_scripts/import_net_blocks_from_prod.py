#!/usr/bin/env python3
"""
Copy a slice of net-tagged blocks from a source micromegas lake (typically prod)
into the local lake so the net_spans view can be exercised end-to-end.

Source connection uses the standard cli/connection.py flow (OIDC if configured).
Target connection is the local flight-sql-srv at grpc://localhost:50051.

For each selected block we pull:
  - processes row          (bulk_ingest via do_put -> replication.ingest_processes)
  - streams   row          (                    -> replication.ingest_streams)
  - blocks    row          (                    -> replication.ingest_blocks)
  - raw payload bytes      (via get_payload() UDF; ingested as payload blob)

Pass --begin/--end to pick the window, or --block-id (repeatable) to import
specific blocks.
"""

import argparse
import datetime as dt
import os
import sys
import time

import pyarrow

# Make sure we pick up the repo's micromegas package (same process running this script).
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
sys.path.insert(0, os.path.join(REPO, "python", "micromegas"))

import micromegas  # noqa: E402
from micromegas.cli import connection as src_connection  # noqa: E402


def _select_net_blocks(src, begin: dt.datetime, end: dt.datetime, explicit_block_ids):
    """Return a pyarrow.Table of blocks-view rows for the selected net blocks."""
    begin_iso = begin.isoformat()
    end_iso = end.isoformat()
    if explicit_block_ids:
        ids_sql = ", ".join(f"'{bid}'" for bid in explicit_block_ids)
        sql = (
            f"SELECT * FROM blocks WHERE block_id IN ({ids_sql}) "
            f"AND begin_time >= TIMESTAMP '{begin_iso}' "
            f"AND begin_time <  TIMESTAMP '{end_iso}'"
        )
    else:
        sql = (
            "SELECT * FROM blocks "
            "WHERE array_contains(\"streams.tags\", 'net') "
            f"AND begin_time >= TIMESTAMP '{begin_iso}' "
            f"AND begin_time <  TIMESTAMP '{end_iso}'"
        )
    batches = list(src.query_stream(sql, begin, end))
    if not batches:
        raise RuntimeError("source query returned zero net block rows")
    return pyarrow.Table.from_batches(batches)


def _select_blocks_for_processes(src, begin, end, process_ids):
    """Return a pyarrow.Table of blocks-view rows for the given processes in [begin,end)."""
    if not process_ids:
        return None
    ids_sql = ", ".join(f"'{pid}'" for pid in process_ids)
    begin_iso = begin.isoformat()
    end_iso = end.isoformat()
    sql = (
        f"SELECT * FROM blocks WHERE process_id IN ({ids_sql}) "
        f"AND begin_time >= TIMESTAMP '{begin_iso}' "
        f"AND begin_time <  TIMESTAMP '{end_iso}'"
    )
    batches = list(src.query_stream(sql, begin, end))
    if not batches:
        raise RuntimeError("process-wide blocks query returned zero rows")
    return pyarrow.Table.from_batches(batches)


def _project(table: pyarrow.Table, columns):
    """Select a subset of columns and strip the 'prefix.' qualifier when present."""
    out_columns = []
    out_names = []
    for src_name, out_name in columns:
        out_columns.append(table.column(src_name))
        out_names.append(out_name)
    return pyarrow.Table.from_arrays(out_columns, names=out_names)


def _build_processes_table(blocks: pyarrow.Table) -> pyarrow.Table:
    """The blocks view already carries every processes.* field we need."""
    projected = _project(
        blocks,
        [
            ("process_id", "process_id"),
            ("processes.exe", "exe"),
            ("processes.username", "username"),
            ("processes.realname", "realname"),
            ("processes.computer", "computer"),
            ("processes.distro", "distro"),
            ("processes.cpu_brand", "cpu_brand"),
            ("processes.tsc_frequency", "tsc_frequency"),
            ("processes.start_time", "start_time"),
            ("processes.start_ticks", "start_ticks"),
            ("processes.insert_time", "insert_time"),
            ("processes.parent_process_id", "parent_process_id"),
            ("processes.properties", "properties"),
        ],
    )
    return _unique_by(projected, "process_id")


def _build_streams_table(blocks: pyarrow.Table) -> pyarrow.Table:
    projected = _project(
        blocks,
        [
            ("stream_id", "stream_id"),
            ("process_id", "process_id"),
            ("streams.dependencies_metadata", "dependencies_metadata"),
            ("streams.objects_metadata", "objects_metadata"),
            ("streams.tags", "tags"),
            ("streams.properties", "properties"),
            ("streams.insert_time", "insert_time"),
        ],
    )
    return _unique_by(projected, "stream_id")


def _build_blocks_table(blocks: pyarrow.Table) -> pyarrow.Table:
    return _project(
        blocks,
        [
            ("block_id", "block_id"),
            ("stream_id", "stream_id"),
            ("process_id", "process_id"),
            ("begin_time", "begin_time"),
            ("begin_ticks", "begin_ticks"),
            ("end_time", "end_time"),
            ("end_ticks", "end_ticks"),
            ("nb_objects", "nb_objects"),
            ("object_offset", "object_offset"),
            ("payload_size", "payload_size"),
            ("insert_time", "insert_time"),
        ],
    )


def _unique_by(table: pyarrow.Table, key: str) -> pyarrow.Table:
    key_col = table.column(key)
    # drop_null would hide bad data; we want to notice it instead.
    if key_col.null_count:
        raise RuntimeError(f"column {key} contains nulls")
    seen = set()
    keep_mask = []
    for i, val in enumerate(key_col.to_pylist()):
        if val in seen:
            keep_mask.append(False)
        else:
            seen.add(val)
            keep_mask.append(True)
    mask = pyarrow.array(keep_mask)
    return table.filter(mask)


def _fetch_payloads(src, begin, end, blocks: pyarrow.Table) -> pyarrow.Table:
    """Download raw payload bytes for the selected blocks using the get_payload UDF."""
    pids = blocks.column("process_id").to_pylist()
    sids = blocks.column("stream_id").to_pylist()
    bids = blocks.column("block_id").to_pylist()
    # Chunk requests so the server doesn't have to buffer everything at once.
    chunk = 8
    tables = []
    for i in range(0, len(bids), chunk):
        triples = list(
            zip(pids[i : i + chunk], sids[i : i + chunk], bids[i : i + chunk])
        )
        values_clause = ", ".join(f"('{p}', '{s}', '{b}')" for (p, s, b) in triples)
        sql = (
            "SELECT process_id, stream_id, block_id, "
            "get_payload(process_id, stream_id, block_id) AS payload "
            f"FROM (VALUES {values_clause}) "
            "AS t(process_id, stream_id, block_id)"
        )
        batches = list(src.query_stream(sql, begin, end))
        if batches:
            tables.append(pyarrow.Table.from_batches(batches))
    if not tables:
        raise RuntimeError("payload fetch returned no rows")
    return pyarrow.concat_tables(tables)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--begin",
        required=True,
        type=lambda s: (
            dt.datetime.fromisoformat(s).replace(tzinfo=dt.timezone.utc)
            if dt.datetime.fromisoformat(s).tzinfo is None
            else dt.datetime.fromisoformat(s)
        ),
    )
    ap.add_argument(
        "--end",
        required=True,
        type=lambda s: (
            dt.datetime.fromisoformat(s).replace(tzinfo=dt.timezone.utc)
            if dt.datetime.fromisoformat(s).tzinfo is None
            else dt.datetime.fromisoformat(s)
        ),
    )
    ap.add_argument(
        "--block-id",
        action="append",
        default=None,
        help="Repeat to pick specific block ids; overrides --begin/--end for the blocks query.",
    )
    ap.add_argument(
        "--target-uri",
        default=os.environ.get(
            "MICROMEGAS_LOCAL_FLIGHTSQL_URL", "grpc://localhost:50051"
        ),
    )
    ap.add_argument(
        "--net-only",
        action="store_true",
        help="Skip importing log/metric/other streams from the same processes.",
    )
    args = ap.parse_args()

    print(f"source: {os.environ.get('MICROMEGAS_ANALYTICS_URI', 'local')}")
    print(f"target: {args.target_uri}")
    print(f"window: {args.begin.isoformat()} → {args.end.isoformat()}")

    src = src_connection.connect()
    target = micromegas.flightsql.client.FlightSQLClient(args.target_uri)

    t0 = time.time()
    net_blocks = _select_net_blocks(src, args.begin, args.end, args.block_id)
    print(f"selected {net_blocks.num_rows} net blocks in {time.time() - t0:.1f}s")

    process_ids = sorted(set(net_blocks.column("process_id").to_pylist()))
    print(f"scoping {len(process_ids)} process(es): {process_ids}")

    t0 = time.time()
    if args.net_only:
        blocks = net_blocks
    else:
        blocks = _select_blocks_for_processes(src, args.begin, args.end, process_ids)
    print(f"selected {blocks.num_rows} blocks total in {time.time() - t0:.1f}s")

    # Summarize tag breakdown so the user can verify logs/metrics came along.
    tags_list = blocks.column("streams.tags").to_pylist()
    tag_counter = {}
    for t in tags_list:
        primary = (t or ["(untagged)"])[0] if t else "(untagged)"
        tag_counter[primary] = tag_counter.get(primary, 0) + 1
    print("blocks per primary tag:", sorted(tag_counter.items()))

    processes = _build_processes_table(blocks)
    streams = _build_streams_table(blocks)
    blocks_out = _build_blocks_table(blocks)

    t0 = time.time()
    payloads = _fetch_payloads(src, args.begin, args.end, blocks_out)
    total_bytes = sum(len(b or b"") for b in payloads.column("payload").to_pylist())
    print(
        f"fetched {payloads.num_rows} payloads "
        f"({total_bytes / 1024 / 1024:.1f} MiB) in {time.time() - t0:.1f}s"
    )

    # Ingest order matters: processes first, streams next, then blocks, then payloads.
    for label, table in [
        ("processes", processes),
        ("streams", streams),
        ("blocks", blocks_out),
        ("payloads", payloads),
    ]:
        result = target.bulk_ingest(label, table)
        n = result.record_count if result is not None else table.num_rows
        print(f"ingested {n} rows into {label}")

    print("done.")


if __name__ == "__main__":
    main()
