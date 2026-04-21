---
marp: true
---

<script type="module">
  import mermaid from 'https://cdn.jsdelivr.net/npm/mermaid@10/dist/mermaid.esm.min.mjs';
  mermaid.initialize({ startOnLoad: true });
</script>

# Micromegas
Unreal Observability

July 2024

https://github.com/madesroches/micromegas/

Marc-Antoine Desroches <madesroches@gmail.com>

---
# Unreal Observability using Micromegas
 * Instrumentation
   * How to add entries to the log
   * How to record metrics
   * How to instrument spans (CPU traces)
   * How to emit span events
 * Analytics
   * Python API
   * Command-line interface (in dev)
   * Perfetto

---
# Instrumentation

 * One header file to #include: `Engine\Source\Runtime\Core\Public\MicromegasTracing\Macros.h`
 * `#include "MicromegasTracing/Macros.h"`

---
# Instrumentation: How to add entries to the log

 * Just use UE_LOG
   * not very efficient, but convenient
 * Don't want to add to unreal's log?
   * MICROMEGAS_LOG_STATIC for static strings (lowest overhead)
   * MICROMEGAS_LOG_DYNAMIC for dynamic strings (much higher overhead, not as bad as UE_LOG)

---
# Instrumentation: How to add entries to the log

```c++
MICROMEGAS_LOG_DYNAMIC("LogMicromegasTelemetrySink", MicromegasTracing::LogLevel::Debug, FString::Printf(TEXT("Sending block %s"), *blockId));


MICROMEGAS_LOG_STATIC("MicromegasTelemetrySink", MicromegasTracing::LogLevel::Info, TEXT("Shutting down"));


```

---
# Instrumentation: How to record metrics

```c++

#define MICROMEGAS_IMETRIC(target, level, name, unit, expr)                                                                                \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::IntMetric(MicromegasTracing::IntegerMetricEvent(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64()))

#define MICROMEGAS_FMETRIC(target, level, name, unit, expr)                                                                                \
	static const MicromegasTracing::MetricMetadata PREPROCESSOR_JOIN(metricMeta, __LINE__)(level, name, unit, target, __FILE__, __LINE__); \
	MicromegasTracing::FloatMetric(MicromegasTracing::FloatMetricEvent(&PREPROCESSOR_JOIN(metricMeta, __LINE__), (expr), FPlatformTime::Cycles64()))

```

```c++

MICROMEGAS_FMETRIC("Frame", MicromegasTracing::Verbosity::Med, TEXT("DeltaTime"), TEXT("seconds"), FApp::GetDeltaTime());

```

---
# Instrumentation: How to instrument spans

 * MICROMEGAS_SPAN_FUNCTION : takes the name of the function for the name of the span
 * MICROMEGAS_SPAN_SCOPE : specify a static name
 * MICROMEGAS_SPAN_NAME : specify an expression that returns a **statically allocated** string for a name
   * works with FNames too :)
   * ```c++
      // DIVERGENCE_BEGIN - engine instrumentation
      MICROMEGAS_SPAN_NAME("Engine::FActorTickFunction::ExecuteTick", Target->GetFName());
      // DIVERGENCE_END```

---
# Instrumentation: How to emit span events

 * Logs and metrics are enabled by default - spans are **NOT**
 * In unreal's console: `telemetry.spans.enable 1`
 * In unreal's console: `telemetry.flush`
 * In unreal's console: `telemetry.spans.enable 0`

---

# Analytics: Python API

 * Some setup required - ask me
 * Ask for code samples of queries that work in your context
 * APIs are for programmers - more accessible tooling is on the way
 
---
<style scoped>
table {
  font-size: 13px;
}
</style>
# Analytics: Python API

Query using SQL

```python
client = micromegas.client.Client("http://localhost:8082/")
sql = """SELECT time, process_id, level, target, msg
FROM log_entries
WHERE level <= 4
AND exe LIKE '%analytics%'
ORDER BY time DESC
LIMIT 2
"""
client.query(sql, begin, end)

```

---

Returns pandas DataFrame

<style scoped>
table {
  font-size: 13px;
}
</style>

|    | time                                | process_id                           |   level | target                                 | msg                                         |
|---:|:------------------------------------|:-------------------------------------|--------:|:---------------------------------------|:--------------------------------------------|
|  0 | 2024-10-03 18:17:56.087543714+00:00 | 1db06afc-1c88-47d1-81b3-f398c5f93616 |       4 | acme_telemetry::trace_middleware       | response status=200 OK uri=/analytics/query |
|  1 | 2024-10-03 18:17:53.924037729+00:00 | 1db06afc-1c88-47d1-81b3-f398c5f93616 |       4 | micromegas_analytics::lakehouse::query | query sql=                                  |
|    |                                     |                                      |         |                                        | SELECT time, process_id, level, target, msg |
|    |                                     |                                      |         |                                        | FROM log_entries                            |
|    |                                     |                                      |         |                                        | WHERE level <= 4                            |
|    |                                     |                                      |         |                                        | AND exe LIKE '%analytics%'                  |
|    |                                     |                                      |         |                                        | ORDER BY time DESC                          |
|    |                                     |                                      |         |                                        | LIMIT 2                                     |

See https://pypi.org/project/micromegas

---
# Analytics: Command-line interface

Install the micromegas Python package to get the CLI tools:

```bash
pip install micromegas
```

```bash
# List recent processes
micromegas-query "SELECT process_id, exe, start_time, username, computer FROM processes ORDER BY start_time DESC LIMIT 10" --begin 15m

# Query log entries
micromegas-query "SELECT time, level, msg FROM log_entries LIMIT 100" --begin 1h
```

---
# Analytics: using perfetto

1. Write trace file using python API
```python
micromegas.perfetto.write_process_trace(client, process_id, "f:/temp/trace.pb")
```

2. Open browser to [https://ui.perfetto.dev](https://ui.perfetto.dev/)

3. Drag & drop trace file into web interface

4. WASD keys to navigate
