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
      // SCOUT_BEGIN - SCOUT-25618 - [DIVERGENCE] engine instrumentation
      MICROMEGAS_SPAN_NAME("Engine::FActorTickFunction::ExecuteTick", Target->GetFName());
      // SCOUT_END```

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
# Analytics: Python API

All methods of client return pandas dataframes or throw errors.

```python

In [2]: import micromegas                                                     
                                                                              
In [3]: client = micromegas.connect()                                         
                                                                              
In [4]: help(client)                                                          
Help on Client in module micromegas.client object:                            
                                                                              
class Client(builtins.object)                                                 
 |  Client(base_url, headers={})                                              
 |                                                                            
 |  Methods defined here:                                                     
 |                                                                            
 |  __init__(self, base_url, headers={})                                      
 |      Initialize self.  See help(type(self)) for accurate signature.        
 |                                                                            
 |  find_process(self, process_id)                                            
 |                                                                            
 |  query_blocks(self, begin, end, limit, stream_id)                          
 |                                                                            
 |  query_log_entries(self, begin, end, limit, stream_id)                     
 |                                                                            
 |  query_metrics(self, begin, end, limit, stream_id)                         
 |                                                                            
 |  query_processes(self, begin, end, limit)                                  
 |                                                                            
 |  query_spans(self, begin, end, limit, stream_id)                           
 |                                                                            
 |  query_streams(self, begin, end, limit, process_id=None, tag_filter=None)  
 |                                                                            
 |  query_thread_events(self, begin, end, limit, stream_id)                   
 |                                                                            
```

---
# Analytics: Command-line interface

Not even pre-alpha - work in progress.

```cmd
F:\git\micromegas\python\micromegas\cli>python query_processes.py --help                                                                                                                       
usage: query_processes [-h] [--since SINCE] [--limit LIMIT]                                                                                                                                    
                                                                                                                                                                                               
List processes in the telemetry database                                                                                                                                                       
                                                                                                                                                                                               
options:                                                                                                                                                                                       
  -h, --help     show this help message and exit                                                                                                                                               
  --since SINCE  [number][m|h|d]                                                                                                                                                               
  --limit LIMIT                                                                                                                                                                                
                                                                                                                                                                                               
If you are in a corporate environment, you may need to set the MICROMEGAS_PYTHON_MODULE_WRAPPER environment variable to specify the python module responsible to authenticate your requests.   
                                                                                                                                                                                               
```

```cmd
F:\git\micromegas\python\micromegas\cli>python query_processes.py --since 15m
hello someone@acme.com
    process_id                            exe                                                                 start_time                        username    computer    distro                                cpu_brand
--  ------------------------------------  ------------------------------------------------------------------  --------------------------------  ----------  ----------  ------------------------------------  -------------------------------------
 0  61db1b72-4330-4335-9e93-5d8fd4f2b661  G:\asdsadasdasdUsdsaasdasdasddddddddddddddddddddddddsddadaator.exe  2024-07-02 18:53:05.545000+00:00  notme       ASDAS3211   WindowsEditor 10                      Intel(R) Xeon(R)
 1  49bfa0da-4c28-4829-27fb-02b90f3b3779  A:\hgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgkjhgr.exe  2024-07-02 18:58:58.228000+00:00  someone     HHFHFHFFF   WindowsClient                         Intel(R) Xeon(R)
 2  81aee033-4ede-04a3-c5ac-edb2a591270d  B:\asdasssssssskjhkjhdkasjdhaskdjhaslkdjhlaKJHDLKjahator.exe        2024-07-02 19:00:18.204000+00:00  asdasdasda  SFDGACVBCV  Windows                               Intel(R) Xeon(R)
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
