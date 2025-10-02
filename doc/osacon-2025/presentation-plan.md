# OSACON 2025 Presentation Plan
## Micromegas - Unified Observability for Video Games

**Event:** The Open Source Analytics Conference (OSACON)
**Date:** November 5, 2025, 19:00-19:30 UTC (30 minutes)
**Speaker:** Marc-Antoine Desroches
**Title:** "Micromegas - unified observability for video games"
**Tagline:** "How we built an open source observability stack that can track every frame of our game"

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

### 1. Hook & Problem Statement (3-4 min)
- Video game performance tracking challenge
- Scale: 100k events/second per process at 60fps
- Why traditional observability tools fail
- Video/screenshots showing the problem

### 2. Architecture Overview (3-4 min)
- Data flow diagram: Instrumentation → Ingestion → Storage → Analytics
- Four main stages to explore
- Unified observability: logs, metrics, traces in one system

### 3. Stage 1: Low-Overhead Instrumentation (4-5 min)
- 20ns overhead per event
- Thread-local storage for high-frequency streams (CPU traces)
- Code examples (Rust, C++, Unreal Engine)
- Transit protocol designed to work with in-memory buffer format

### 4. Stage 2: Scalable Ingestion (1-2 min)
- HTTP service accepting compressed payloads
- Metadata → PostgreSQL, payloads → S3/GCS
- Simple, horizontally scalable design

### 5. Stage 3: Lakehouse Storage (3-4 min)
- Datalake: cheap writes (custom format, object storage)
- Lakehouse: fast reads (Parquet, columnar)
- Just-in-time ETL and tail sampling

### 6. Stage 4: SQL Analytics (4-5 min)
- Apache Arrow FlightSQL for queries
- DataFusion SQL engine
- Incremental data reduction with SQL-defined views
- Cost comparison with traditional approaches

### 7. Examples & Results (4-5 min)
- Screenshots/video of Perfetto trace viewer
- SQL query examples and results
- Performance metrics from production use
- Cost savings examples

### 8. Open Source & Community (2-3 min)
- GitHub project overview
- Technology stack (Rust, DataFusion, Arrow, PostgreSQL)
- Current integrations (Rust, Python, Unreal Engine)
- Contribution opportunities

### 9. Q&A (2-3 min)
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
