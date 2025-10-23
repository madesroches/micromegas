# Async Events Performance Analysis

This directory contains tools and reports for analyzing async events performance in Micromegas.

## Files

- `async_events_analyzer.py` - Python script for automated performance analysis
- `async_events_performance_analysis.md` - Detailed performance report example
- `README.md` - This documentation file

## Usage

### Prerequisites

1. **Start Micromegas services:**
   ```bash
   python3 local_test_env/ai_scripts/start_services.py
   ```

2. **Ensure you're in the poetry environment:**
   ```bash
   cd python/micromegas
   poetry shell
   ```

### Running the Analyzer

#### Quick Summary
Get a concise overview of performance bottlenecks:

```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py <process_id> --summary-only
```

**Example:**
```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py 1333745d-77e3-4399-b937-c2562d9f526f --summary-only
```

#### Full Analysis
Get detailed performance breakdown with all metrics including flame graphs:

```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py <process_id>
```

**Example:**
```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py 1333745d-77e3-4399-b937-c2562d9f526f
```

#### Flame Graphs Only
Generate only flame graph visualizations without console analysis:

```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py <process_id> --flame-only
```

#### Skip Flame Graphs
Run analysis without generating flame graphs (faster):

```bash
poetry run python ../../tasks/perf_report/async_events_analyzer.py <process_id> --no-flame-graphs
```

### Finding Process IDs

To find available process IDs for analysis:

```python
import micromegas
client = micromegas.connect()

# List all processes
processes = client.query("SELECT DISTINCT process_id FROM processes")
print(processes)

# Or check recent async events
recent = client.query("""
    SELECT DISTINCT stream_id 
    FROM async_events 
    ORDER BY time DESC 
    LIMIT 10
""")
print(recent)
```

## Analysis Output

### Summary Mode Output
```
KEY FINDINGS SUMMARY
==================================================

Dataset: 2374 events across 1912 spans

Performance Bottleneck:
  🔥 fetch_overlapping_insert_range_for_view: 612.04ms (58.6% of total time)
  
Concurrency:
  📈 Peak: 96 simultaneous operations
  
Top Optimization Target:
  The fetch_overlapping_insert_range_for_view operation shows the highest total duration
  and should be the primary focus for performance improvements.
```

### Full Analysis Sections

1. **📊 Dataset Overview** - Event counts, spans, time ranges
2. **📋 Span Names Distribution** - Most frequent operations by event count
3. **⚡ Performance by Span Name** - Duration statistics and bottlenecks
4. **🐌 Slowest Individual Spans** - Top 10 longest-running operations
5. **🚦 Concurrency Analysis** - Parallel execution patterns
6. **🔥 Flame Graph Generation** - Visual performance profiling data
7. **Summary** - Key findings and optimization targets

## Flame Graph Output

The analyzer generates three types of flame graph files:

### 1. Brendan Gregg Format (.txt)
```
query;make_session_context 4833
partition_cache;fetch_overlapping_insert_range_for_view 2747
payload;fetch_block_payload 94
```
- **Usage:** Compatible with [flamegraph.pl](https://github.com/brendangregg/FlameGraph)
- **Generate SVG:** `cat flame_data.txt | flamegraph.pl > flame.svg`

### 2. JSON Format (.json)
```json
{
  "name": "root",
  "children": [
    {
      "name": "query",
      "value": 268668,
      "children": [{"name": "make_session_context", "value": 268668}]
    }
  ]
}
```
- **Usage:** Compatible with d3-flame-graph and web-based visualizers
- **Interactive:** Can be loaded into online flame graph tools

### 3. HTML Visualization (.html)
- **Immediate viewing:** Self-contained HTML file with inline visualization
- **Performance bars:** Shows relative operation durations
- **Statistics table:** Detailed breakdown with percentages

## Generating SVG Flame Graphs

For traditional flame graph visualization, use the included helper script:

```bash
./generate_svg_flame.sh <process_id>
```

**Requirements:**
- Install [FlameGraph tools](https://github.com/brendangregg/FlameGraph): `git clone https://github.com/brendangregg/FlameGraph.git`
- Add `flamegraph.pl` to your PATH

**Manual generation:**
```bash
# Generate data
poetry run python ../../tasks/perf_report/async_events_analyzer.py <process_id> --flame-only

# Create SVG
cat flame_graphs/async_events_*.txt | flamegraph.pl --title "Async Events Performance" > flame.svg
```

## Understanding the Results

### Key Metrics

- **Total Duration** - Most important metric; shows cumulative impact
- **Average Duration** - Indicates per-operation efficiency  
- **Standard Deviation** - Shows consistency (lower = more predictable)
- **Max Duration** - Identifies outlier performance issues

### Performance Priorities

Focus optimization efforts on operations with:
1. **High total duration** (biggest overall impact)
2. **High standard deviation** (inconsistent performance)
3. **High execution count** (frequent operations)

### Common Bottlenecks

Based on analysis, typical performance issues include:

- **`fetch_overlapping_insert_range_for_view`** - Partition cache inefficiencies
- **`make_session_context`** - Query session initialization overhead
- **`fetch_block_payload`** - Data retrieval operations

## Automation & Monitoring

### Continuous Monitoring
Run the analyzer regularly to track performance trends:

```bash
# Daily performance check
poetry run python ../../tasks/perf_report/async_events_analyzer.py $(latest_process_id) --summary-only >> daily_perf.log
```

### Integration with CI/CD
Add performance regression detection:

```bash
# In CI pipeline - fail if performance degrades
python async_events_analyzer.py $PROCESS_ID --summary-only | grep "fetch_overlapping" | awk '{if($3 > 700.0) exit 1}'
```

### Alerting Thresholds

Based on baseline analysis, consider alerting when:
- `fetch_overlapping_insert_range_for_view` > 700ms total time
- `make_session_context` > 300ms total time  
- Any operation shows > 10ms average duration
- Max concurrent spans > 100

## Troubleshooting

### Common Issues

**"Module not found" error:**
```bash
# Ensure you're in the poetry environment
cd python/micromegas
poetry shell
```

**"Flight returned unavailable" error:**
```bash
# Start the services
python3 ../../local_test_env/ai_scripts/start_services.py
# Wait a few seconds for services to initialize
```

**"No data found" error:**
- Verify the process ID exists in your data
- Check that async events are being generated
- Ensure the time range contains data

### Debug Mode
For detailed SQL query debugging, modify the script to print queries:

```python
# Add before client.query(sql) calls:
print(f"DEBUG SQL: {sql}")
```

## Contributing

When adding new analysis capabilities:

1. **Add new analysis methods** to the `AsyncEventsAnalyzer` class
2. **Update the `run_analysis()`** method to include new analyses  
3. **Add corresponding print methods** for output formatting
4. **Update this README** with new features and usage examples

Example new analysis method:
```python
def _analyze_error_patterns(self) -> pd.DataFrame:
    """Analyze error patterns in async events."""
    sql = f"""
    SELECT name, COUNT(*) as error_count
    FROM view_instance('async_events', '{self.process_id}')
    WHERE event_type = 'error'
    GROUP BY name
    ORDER BY error_count DESC
    """
    return self.client.query(sql)
```