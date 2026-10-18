#!/usr/bin/env python3
"""Turns criterion estimates and the saved bench stdout into the markdown tables of
mkdocs/docs/when-to-use/instrumentation-overhead.md.

Usage (from rust/):
    cargo bench -p micromegas-otel-comparison-bench | tee bench.log
    python3 otel-comparison-bench/report.py bench.log [--criterion-dir target/criterion]

Stdlib only. Group, case, library and thread count come from the ids criterion records in
`benchmark.json`, never from directory names: criterion replaces `/` with `_` there, and case
and library names contain `_`.
"""

import argparse
import json
import re
import sys
from pathlib import Path

LIBS = {
    "micromegas",
    "micromegas_interop",
    "otel",
    "otel_tracing",
    "otel_gzip",
}
HOT_PATH_GROUPS = {
    "log": ["micromegas", "micromegas_interop", "otel"],
    "span": ["micromegas", "otel", "otel_tracing"],
}
CASE_ORDER = {
    "log": ["static", "formatted", "static_attrs", "dyn_low_card", "dyn_high_card"],
    "span": ["static", "attrs", "dyn_name"],
}
CONTENTION_ORDER = ["log/static", "log/dyn_high_card", "span/static"]
CONTENTION_LIB_ORDER = ["micromegas", "otel"]
ENCODE_ORDER = [
    "log_static_attrs",
    "log_dyn_high_card",
    "span_static",
    "span_dyn_name",
]
SERIES_COUNTS = [10, 100]
THREADS = [1, 4, 8]

BYTES_RE = re.compile(r"^bytes: (\S+) events=(\d+) raw=(\d+) wire=(\d+)\s*$")
MISSING = "—"


def load_estimates(criterion_dir):
    """Returns {(group, case, lib, threads or None): (median ns, elements or None)}."""
    results = {}
    for bench_json in criterion_dir.rglob("new/benchmark.json"):
        estimates_json = bench_json.with_name("estimates.json")
        if not estimates_json.exists():
            continue
        ident = json.loads(bench_json.read_text())
        median = json.loads(estimates_json.read_text())["median"]["point_estimate"]
        group = ident["group_id"]
        function_id = ident["function_id"]
        value = ident.get("value_str")
        if function_id is None or value is None:
            continue
        if group == "contention":
            case, _, lib = function_id.rpartition("/")
            threads = int(value)
        else:
            case, lib, threads = function_id, value, None
        if lib not in LIBS:
            # rust/tracing/benches/properties.rs writes the same group names.
            continue
        elements = (ident.get("throughput") or {}).get("Elements")
        results[(group, case, lib, threads)] = (median, elements)
    return results


def load_sizes(path):
    sizes = {}
    for line in Path(path).read_text().splitlines():
        m = BYTES_RE.match(line)
        if m:
            sizes[m.group(1)] = tuple(int(g) for g in m.groups()[1:])
    return sizes


def table(header, rows):
    lines = ["| " + " | ".join(header) + " |", "|" + "|".join(["---"] * len(header)) + "|"]
    lines += ["| " + " | ".join(row) + " |" for row in rows]
    return "\n".join(lines)


def hot_path(results):
    out = []
    for group, libs in HOT_PATH_GROUPS.items():
        rows = []
        for case in CASE_ORDER[group]:
            row = [f"`{case}`"]
            for lib in libs:
                est = results.get((group, case, lib, None))
                if est is None:
                    row.append(MISSING)
                else:
                    row.append(f"{est[0]:.1f}")
            rows.append(row)
        out.append(f"### `{group}`: median ns per operation\n")
        header = ["case"] + [f"`{lib}`" for lib in libs]
        out.append(table(header, rows))
        out.append("")
    return "\n".join(out)


def contention(results):
    rows = []
    for case in CONTENTION_ORDER:
        for lib in CONTENTION_LIB_ORDER:
            if not any((("contention", case, lib, t) in results) for t in THREADS):
                continue
            row = [f"`{case}`", f"`{lib}`"]
            for threads in THREADS:
                est = results.get(("contention", case, lib, threads))
                if est is None:
                    row.append(MISSING)
                    continue
                mops = threads / est[0] * 1000.0
                row.append(f"{mops:.1f}")
            rows.append(row)
    header = ["case", "lib"] + [f"{t} thread{'s' if t > 1 else ''}" for t in THREADS]
    return "### `contention`: aggregate Mops/s\n\n" + table(header, rows) + "\n"


def encode(results, sizes):
    time_rows, size_rows = [], []
    for case in ENCODE_ORDER:
        mm = sizes.get(f"encode/{case}/micromegas")
        otel = sizes.get(f"encode/{case}/otel")
        row = [f"`{case}`"]
        for lib in ["micromegas", "otel", "otel_gzip"]:
            est = results.get(("encode", case, lib, None))
            if est is None or not est[1]:
                row.append(MISSING)
            else:
                row.append(f"{est[0] / est[1]:.1f}")
        time_rows.append(row)
        size_rows.append(
            [f"`{case}`"]
            + [f"{v[i] / v[0]:.1f}" if v else MISSING for v, i in [(mm, 1), (mm, 2), (otel, 1), (otel, 2)]]
        )
    out = ["### `encode`: ns per event, off the calling thread\n"]
    out.append(table(["case", "`micromegas`", "`otel`", "`otel_gzip`"], time_rows))
    out.append("\n### Bytes per event\n")
    out.append(
        table(
            [
                "case",
                "`micromegas` raw",
                "`micromegas` wire (LZ4 + CBOR)",
                "`otel` protobuf",
                "`otel_gzip`",
            ],
            size_rows,
        )
    )
    return "\n".join(out) + "\n"


def metric_batch(results, sizes):
    """One batch is one value for each of N series. Micromegas sizes are per batch of a block
    holding `events` batches; OpenTelemetry sizes are for one collected batch."""

    def median(case, lib):
        est = results.get(("metric_batch", case, lib, None))
        return None if est is None else est[0] / (est[1] or 1)

    def cell(value):
        return MISSING if value is None else f"{value:.1f}"

    record_rows, export_rows, size_rows = [], [], []
    for n in SERIES_COUNTS:
        mm = median(f"record/{n}", "micromegas")
        otel = median(f"record/{n}", "otel")
        record_rows.append(
            [str(n)] + [cell(v) for v in (mm, otel)]
            + [cell(v / n if v is not None else None) for v in (mm, otel)]
        )
        export_rows.append(
            [str(n)] + [cell(median(f"export/{n}", lib)) for lib in ("micromegas", "otel", "otel_gzip")]
        )
        s_mm = sizes.get(f"metric_batch/{n}/micromegas")
        s_otel = sizes.get(f"metric_batch/{n}/otel")
        size_rows.append(
            [str(n)]
            + [
                f"{v[i] / v[0]:.1f}" if v else MISSING
                for v, i in [(s_mm, 1), (s_mm, 2), (s_otel, 1), (s_otel, 2)]
            ]
        )
    out = ["### `metric_batch`: median ns per batch on the calling thread\n"]
    out.append(
        table(
            ["series per batch", "`micromegas`", "`otel`", "`micromegas` per series", "`otel` per series"],
            record_rows,
        )
    )
    out.append("\n### `metric_batch`: median ns per batch, off the calling thread\n")
    out.append(table(["series per batch", "`micromegas`", "`otel`", "`otel_gzip`"], export_rows))
    out.append("\n### Bytes per batch\n")
    out.append(
        table(
            [
                "series per batch",
                "`micromegas` raw",
                "`micromegas` wire (LZ4 + CBOR)",
                "`otel` protobuf",
                "`otel_gzip`",
            ],
            size_rows,
        )
    )
    return "\n".join(out) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("bench_log", help="saved stdout of the bench run")
    default_dir = Path(__file__).resolve().parent.parent / "target" / "criterion"
    parser.add_argument("--criterion-dir", type=Path, default=default_dir)
    args = parser.parse_args()

    results = load_estimates(args.criterion_dir)
    if not results:
        sys.exit(f"no criterion results under {args.criterion_dir}")
    sizes = load_sizes(args.bench_log)
    print(hot_path(results))
    print(contention(results))
    print(encode(results, sizes))
    print(metric_batch(results, sizes))


if __name__ == "__main__":
    main()
