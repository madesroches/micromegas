# High-Frequency Observability Presentation Plan
## Micromegas - Cost-Efficient Telemetry at Scale

**Speaker:** Marc-Antoine Desroches (madesroches@gmail.com)
**Title:** "High-Frequency Observability: Cost-Efficient Telemetry at Scale"
**Tagline:** "How to record more data for less money with tail sampling and lakehouse architecture"

---

## Key Message
**More efficient telemetry pipelines are possible - you can record more data for less money.**

Micromegas achieves this through multiple strategies:

1. **Low-overhead instrumentation** - 20ns overhead per event enables high-frequency collection (100k+ events/sec)
2. **Cheap storage strategy** - Raw payloads in object storage (S3/GCS), only metadata in PostgreSQL
3. **Tail sampling & just-in-time ETL** - Delay the decision to process or delete data, process only when querying
4. **Lakehouse architecture** - Bridge between datalake (cheap writes) and data warehouse (fast reads)
5. **Incremental data reduction** - SQL-defined views that progressively aggregate data

---

## Target Audience
- Data engineers and analytics practitioners
- People working with high-frequency data (IoT, financial trading, scientific computing)
- Platform engineers managing observability infrastructure
- Open source enthusiasts interested in cost-efficient telemetry

---

## Presentation Structure (30 min)

### 1. Hook & Problem Statement (3 min)
- Video game performance tracking challenge
- Scale: 100k events/second per process at 60fps
- Thousands of concurrent processes
- Unified observability: logs, metrics, traces in one system
- Problem: High-frequency tools are typically debugging tools, not analytics tools
- This requires reproducing problems to investigate them
- We refuse to choose between debugging at high frequency and recording at low frequency

### 2. Architecture Overview (2 min)
- Data flow diagram: Instrumentation → Ingestion (lake) → Analytics (lakehouse) → UI
- Four main stages to explore

### 3. Stage 1: Low-Overhead Instrumentation (4 min)
- 20ns overhead per event
- Thread-local storage for high-frequency streams (CPU traces)
- Code examples (Rust, C++, Unreal Engine)
- Transit protocol designed to work with in-memory buffer format

### 4. Stage 2: Ingestion & Lake Storage (2 min)
- HTTP service accepting compressed payloads
- Metadata → PostgreSQL, payloads → S3/GCS
- Simple, horizontally scalable design
- Datalake: cheap writes (custom format, object storage)

### 5. Stage 3: SQL Analytics (5 min)
- **Lakehouse architecture**: Bridge between lake (cheap writes) and warehouse (fast reads)
  - Raw data stored in custom format for efficiency
  - Transformed to Parquet (columnar) for analytics
- **Tail sampling strategy by stream frequency**:
  - Logs (low frequency): Process eagerly to Parquet
  - Metrics (medium frequency): Process eagerly to Parquet
  - CPU traces (very high frequency): Keep in raw format, process just-in-time when queried
- **Query interface**: Apache Arrow FlightSQL with DataFusion SQL engine
- **Incremental data reduction**: SQL-defined views that progressively aggregate data

### 6. User Interface (4 min)
- **Notebooks**: Jupyter integration for data exploration
- **Grafana**: Dashboards and alerting via plugin (screenshot)
- **Perfetto**: Trace viewer for detailed performance analysis
- SQL query examples and results

### 7. Operating Costs (3 min)
- **Real production example**: 449 billion events over 90 days for ~$1,000/month
  - 8.5 TB storage, 9B logs, 275B metrics, 165B traces
  - ~1,900 events/second average throughput
- **Cost breakdown**: Compute (~$300), PostgreSQL (~$200), S3 (~$500)
- **Tail sampling advantage**: Store everything cheaply, process on-demand
- **Cost comparison**: Orders of magnitude cheaper than commercial SaaS at scale

### 8. Thank You & Open Source (2 min)
- **Would not be possible without open source**:
  - Apache Arrow, Parquet, FlightSQL, DataFusion
  - PostgreSQL
- **Micromegas is open source**: https://github.com/madesroches/micromegas
  - Drop a star, always makes my day

### 9. Q&A (5 min)
- Open floor for questions
- Backup slides ready for common questions

---

## Key Technical Points to Cover

### Performance
- 20ns overhead per event (instrumentation)
- 100k events/second peak throughput
- Thread-local storage for high-frequency streams

### Storage & Cost
- Raw data in cheap object storage (native application format)
- Transformed data in Parquet (columnar, fast queries)
- Metadata in PostgreSQL for fast lookups
- On-demand ETL vs continuous processing

### Query Capabilities
- SQL via Apache Arrow FlightSQL
- Python API for data science workflows
- Grafana plugin for dashboards
- Perfetto integration for trace visualization

### Developer Experience
- Simple instrumentation APIs
- Familiar SQL for analytics

---

## Visual Assets Needed

1. **Architecture Diagram**
   - Data flow: App → Ingestion → Storage → Analytics
   - Technology stack visualization

2. **Performance Comparison**
   - Traditional TSDB vs Micromegas
   - Event volume chart (60fps game data)

3. **Code Examples**
   - Rust instrumentation
   - Unreal Engine macros
   - SQL queries

4. **Demo Screenshots/Video**
   - Perfetto trace viewer
   - Grafana dashboard
   - Query results
   - Live metrics

5. **Storage Cost Comparison**
   - Object storage vs traditional TSDB pricing

---

## Visual Content Strategy

### Screenshots and Videos to Include
- **Problem illustration**: Chart showing 60fps event volume
- **Code examples**: Rust/C++ instrumentation snippets
- **Architecture diagram**: Full data flow visualization
- **Perfetto traces**: Pre-captured game performance analysis
- **Grafana dashboard**: Monitoring and alerting example
- **SQL queries**: Examples with results shown
- **Cost comparison**: Visual chart of storage costs

---

## Backup Content (if time allows)

- Comparison with other observability tools (Jaeger, Prometheus, etc.)
- More advanced SQL queries
- Materialized views and optimization
- Enterprise deployment considerations

---

## Call to Action

- Star the repo
- Try Micromegas: https://github.com/madesroches/micromegas
- Contribute (especially new language integrations)
- Join community discussions
- Share use cases and feedback

---

## Technical Requirements

- All content embedded in presentation

---

## Risk Mitigation

- **Time runs over:** Skip backup content
- **Time runs short:** Have concise summary ready
- **Technical questions:** Prepare FAQ slides

---

## Success Metrics

- Clear problem/solution understanding
- Audience tries Micromegas after talk
- GitHub stars increase
- Community engagement (issues, PRs)
- Follow-up questions and discussions
