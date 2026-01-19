# Live Telemetry Client Evaluation for Micromegas

## Executive Summary

This document evaluates Ubisoft's **Live Telemetry Client** (.NET library) for potential integration with the Micromegas observability platform.

**Key Findings:**
- ✅ Live Telemetry is a mature .NET library for real-time telemetry collection
- ✅ Optimized for Ubisoft's internal infrastructure with Protocol Buffers + Snappy compression
- ⚠️ Technology stack mismatch (.NET vs Rust/Python/TypeScript in Micromegas)
- ⚠️ Designed for different use cases (game telemetry vs observability platform)
- ❌ **Architectural overlap** - Live Telemetry and Micromegas serve similar purposes but with different approaches
- ❌ **Separate infrastructure** - Integration would require running both systems

---

## Overview of Live Telemetry Client

### What is Live Telemetry?

**Type:** .NET Standard 2.0 client library for telemetry collection and export

**Purpose:** Collect, encode, compress, and export real-time telemetry data from games and applications to Ubisoft's Live Telemetry infrastructure

**Key Components:**
- **Client Library**: .NET Standard 2.0 library for data collection
- **Protocol Buffers**: Serialization format (v1 and latest)
- **Snappy Compression**: High-performance compression
- **Multiple Exporters**: HTTP, File, LocalStack (VictoriaMetrics)
- **Multiple Environments**: Internal, External, Staging, Local development

**Architecture Pattern:**
```
Game/Application (.NET)
    ↓
LiveTelemetry.Client (collect + encode + compress)
    ↓
Export (HTTP/File)
    ↓
Live Telemetry Infrastructure (Ubisoft internal)
    ↓
VictoriaMetrics / Databricks storage
```

---

## Live Telemetry Features

### Data Collection
- ✅ **Measurement Types**: `long`, `double`, `string`, `object`
- ✅ **Instruments**: Single and multiple measurement instruments
- ✅ **Custom Timestamps**: Configurable timestamp conversion (Stopwatch-based or custom)
- ✅ **Category Support**: Organize measurements by category

### Compression and Encoding
- ✅ **Protocol Buffers**: Binary serialization for efficiency
- ✅ **Snappy Compression**: Fast compression optimized for telemetry
- ✅ **xxHash**: Data integrity verification
- ✅ **Mapping Dictionary**: String optimization for repeated values

### Export Options
- ✅ **HTTP Exporter**: Send to Live Telemetry servers (internal/external)
- ✅ **File Exporter**: Local JSON file output for debugging
- ✅ **LocalStack**: VictoriaMetrics integration for local development
- ✅ **Retry Policy**: Automatic retry for HTTP failures

### Environments
- ✅ **Internal** (`https://internal.lvtele.ubisoft.org/`) - Default for C# tools
- ✅ **External** (UbiServices endpoints) - Production games
- ✅ **Staging** (`https://uat-internal.lvtele.ubisoft.org/`) - Testing
- ✅ **LocalStack** (`http://localhost:8086/`) - Local VictoriaMetrics

---

## Comparison with Micromegas

| Feature | Live Telemetry | Micromegas |
|---------|---------------|------------|
| **Language** | .NET Standard 2.0 (C#) | Rust (core), Python, TypeScript |
| **Use Case** | Game telemetry collection | Unified observability (logs, metrics, traces) |
| **Data Format** | Protocol Buffers + Snappy | Apache Arrow + Parquet |
| **Instrumentation** | Manual instruments | Rust macros (#[span_fn], etc.) |
| **Storage** | VictoriaMetrics / Databricks | PostgreSQL + Object Storage (S3/GCS) |
| **Query** | VictoriaMetrics PromQL / Databricks SQL | FlightSQL + DataFusion |
| **Compression** | Snappy (fast) | Parquet (columnar, highly compressed) |
| **Transport** | HTTP with retry | HTTP (ingestion) |
| **Performance** | Optimized for metrics | Optimized for high-frequency events (100k/s) |
| **Infrastructure** | Ubisoft Live Telemetry servers | Self-hosted or cloud |
| **License** | Proprietary (Ubisoft internal) | Open source components |
| **Telemetry Types** | Primarily metrics (long/double) | Logs, metrics, spans (unified) |
| **Overhead** | Low (.NET library) | Very low (20ns per event in Rust) |
| **Observability Model** | Metrics-focused | OpenTelemetry-inspired (unified) |

---

## Pros of Live Telemetry

### Infrastructure & Integration
- ✅ **Ubisoft Production-Ready**: Battle-tested in AAA games
- ✅ **Managed Infrastructure**: Ubisoft handles servers, storage, scaling
- ✅ **Multi-Environment Support**: Dev, staging, production endpoints
- ✅ **Authentication**: Integrated with UbiServices (session tickets)

### Performance
- ✅ **Optimized Encoding**: Protocol Buffers binary format
- ✅ **Fast Compression**: Snappy for low CPU overhead
- ✅ **Efficient Transport**: Batched HTTP with retry
- ✅ **Low Memory**: String dictionary optimization

### Developer Experience
- ✅ **NuGet Package**: Easy installation for .NET projects
- ✅ **Factory Methods**: Simple client creation (CreateInternal, CreateExternal, etc.)
- ✅ **Local Development**: LocalStack with VictoriaMetrics for testing
- ✅ **JSON Export**: Debugging via local JSON files
- ✅ **Documentation**: Clear examples and API docs

### Standardization
- ✅ **Ubisoft Standard**: Used across multiple game teams
- ✅ **Consistent Format**: Standardized telemetry data model
- ✅ **Centralized Storage**: All game telemetry in one place

---

## Cons of Live Telemetry

### Technology Stack Mismatch
- ❌ **.NET Only**: Client library is .NET Standard 2.0
- ❌ **No Rust Support**: Micromegas core is Rust-based
- ❌ **No TypeScript SDK**: Web apps use TypeScript
- ❌ **Language Barrier**: Would need language bindings or HTTP API only

### Architectural Overlap
- ❌ **Duplicate Functionality**: Both collect and export telemetry
- ❌ **Separate Infrastructure**: Running both systems is redundant
- ❌ **Different Data Models**: Protocol Buffers vs Arrow/Parquet
- ❌ **Storage Duplication**: VictoriaMetrics + PostgreSQL/Object Storage

### Use Case Mismatch
- ❌ **Metrics-Focused**: Primarily for numeric measurements
- ❌ **Limited Logs**: Not designed for structured log messages
- ❌ **No Span Support**: Doesn't track distributed traces/spans
- ❌ **Game Telemetry Focus**: Optimized for game metrics, not observability

### Infrastructure Dependency
- ❌ **Ubisoft-Specific**: Requires Ubisoft Live Telemetry infrastructure
- ❌ **Network Requirement**: Internal endpoint needs Ubisoft network
- ❌ **Authentication**: Requires UbiServices session tickets
- ❌ **Proprietary**: Not open source, limited to Ubisoft

### Integration Complexity
- ❌ **Different Transport**: HTTP export vs Micromegas HTTP ingestion (different formats)
- ❌ **Schema Mismatch**: Protocol Buffers instruments vs Micromegas events
- ❌ **Timestamp Systems**: Different timestamp conversion approaches
- ❌ **No Direct Compatibility**: Cannot directly consume Live Telemetry data in Micromegas

### Limited Observability
- ❌ **No Unified View**: Metrics only, no logs or traces
- ❌ **Separate Analysis**: Can't correlate with other observability data
- ❌ **Different Query Language**: PromQL/SQL vs FlightSQL/DataFusion

---

## Integration Scenarios

### Scenario 1: Bridge - Export Live Telemetry to Micromegas

**Approach:** Create a bridge service that consumes Live Telemetry exports and forwards to Micromegas

**Architecture:**
```
.NET Application
    ↓
LiveTelemetry.Client (CreateLocalStack)
    ↓
VictoriaMetrics (localhost:8086)
    ↓
Custom Bridge Service (read VictoriaMetrics metrics)
    ↓
Transform to Micromegas format
    ↓
Micromegas Ingestion (HTTP)
    ↓
PostgreSQL + Object Storage
```

**Pros:**
- ✅ Can use existing .NET applications with Live Telemetry
- ✅ Centralize all telemetry in Micromegas for unified analysis
- ✅ Leverage Live Telemetry's .NET SDK

**Cons:**
- ❌ Complex bridge service (VictoriaMetrics → Micromegas format conversion)
- ❌ Additional infrastructure (VictoriaMetrics)
- ❌ Latency and overhead
- ❌ Data transformation complexity (PromQL metrics → Micromegas events)
- ❌ Maintain bridge service code
- ❌ Loses real-time benefits (delayed by bridge processing)

**Effort:** High (2-3 weeks)
- VictoriaMetrics setup: 1-2 days
- Bridge service development: 1-2 weeks
- Format transformation logic: 3-5 days
- Testing and deployment: 2-3 days

**Recommendation:** ⭐ Not recommended - too much complexity for limited benefit

---

### Scenario 2: Dual System - Run Both Independently

**Approach:** Use Live Telemetry for game metrics, Micromegas for observability

**Architecture:**
```
.NET Game
    ↓
LiveTelemetry.Client → Live Telemetry Infrastructure (game metrics)

Rust/Python/TypeScript Services
    ↓
Micromegas Tracing → Micromegas (logs, traces, metrics)
```

**Pros:**
- ✅ Each system optimized for its use case
- ✅ No integration complexity
- ✅ Leverage existing Live Telemetry infrastructure for games
- ✅ Use Micromegas for service observability

**Cons:**
- ❌ Separate systems to maintain
- ❌ Duplicate infrastructure
- ❌ Cannot correlate game metrics with service logs/traces
- ❌ Different query interfaces (VictoriaMetrics vs FlightSQL)
- ❌ Fragmented observability

**Effort:** Low (already separate)

**Recommendation:** ⭐⭐ Acceptable if games must use Live Telemetry

---

### Scenario 3: Replace Live Telemetry with Micromegas

**Approach:** Use Micromegas for all telemetry, deprecate Live Telemetry client

**Architecture:**
```
.NET Game
    ↓
Micromegas .NET Client (need to create)
    ↓
Micromegas Ingestion (HTTP)
    ↓
PostgreSQL + Object Storage
    ↓
FlightSQL Queries
```

**Pros:**
- ✅ Unified observability platform
- ✅ Single infrastructure to maintain
- ✅ Correlate game metrics with service logs/traces
- ✅ Consistent data model (Arrow/Parquet)
- ✅ Single query interface (FlightSQL)

**Cons:**
- ❌ **Need to build .NET client for Micromegas** (significant work)
- ❌ Migrate existing Live Telemetry users
- ❌ Lose Ubisoft-managed infrastructure
- ❌ Need to self-host Micromegas at scale
- ❌ Change management (teams using Live Telemetry)

**Effort:** Very High (2-3 months)
- .NET client library: 4-6 weeks
- Migration tooling: 2-3 weeks
- Testing with games: 2-3 weeks
- Rollout and support: 2-4 weeks

**Recommendation:** ⭐⭐⭐ Best long-term solution if unified observability is priority

---

### Scenario 4: Micromegas Ingestion API for Live Telemetry Format

**Approach:** Add Live Telemetry Protocol Buffers ingestion endpoint to Micromegas

**Architecture:**
```
.NET Application
    ↓
LiveTelemetry.Client (custom endpoint)
    ↓
Micromegas Ingestion Server
    ├── HTTP endpoint (existing Arrow format)
    └── NEW: Protocol Buffers endpoint (Live Telemetry format)
    ↓
Transform to Micromegas internal format
    ↓
PostgreSQL + Object Storage
```

**Pros:**
- ✅ No need for .NET client rewrite
- ✅ Use existing Live Telemetry SDK in .NET apps
- ✅ Unified storage in Micromegas
- ✅ Can mix Live Telemetry apps with Micromegas apps

**Cons:**
- ❌ Need to implement Protocol Buffers decoder in Micromegas (Rust)
- ❌ Support multiple ingestion formats (complexity)
- ❌ Mapping Live Telemetry instruments to Micromegas events
- ❌ Different timestamp systems to reconcile
- ❌ Maintenance burden (two ingestion formats)

**Effort:** High (3-4 weeks)
- Protocol Buffers schema import: 2-3 days
- Rust decoder implementation: 1-2 weeks
- Format transformation logic: 1 week
- Testing and validation: 3-5 days

**Recommendation:** ⭐⭐⭐⭐ Good compromise for .NET application support

---

### Scenario 5: HTTP Bridge - Use FileExporter + Parser

**Approach:** Export Live Telemetry to JSON files, parse and forward to Micromegas

**Architecture:**
```
.NET Application
    ↓
LiveTelemetry.Client.CreateLocalJson()
    ↓
JSON files on disk
    ↓
File Watcher Service
    ↓
Parse JSON + Transform
    ↓
Micromegas Ingestion (HTTP)
```

**Pros:**
- ✅ Simplest integration approach
- ✅ No network dependencies
- ✅ Easy debugging (human-readable JSON)
- ✅ No protocol buffer parsing needed

**Cons:**
- ❌ File I/O overhead
- ❌ Not suitable for high-frequency telemetry
- ❌ File system coupling
- ❌ Delayed ingestion (file watcher latency)
- ❌ Disk space consumption

**Effort:** Medium (1 week)
- File watcher service: 2-3 days
- JSON parsing + transformation: 2-3 days
- Testing: 1-2 days

**Recommendation:** ⭐⭐ Acceptable for low-frequency telemetry or prototyping

---

## Decision Matrix

| Scenario | Effort | Complexity | Unification | .NET Support | Recommendation |
|----------|--------|------------|-------------|--------------|----------------|
| **1. VictoriaMetrics Bridge** | High (2-3w) | Very High | Partial | ✅ Yes | ⭐ Not recommended |
| **2. Dual System** | Low | Low | ❌ No | ✅ Yes | ⭐⭐ Acceptable |
| **3. Replace with Micromegas** | Very High (2-3m) | High | ✅ Complete | ⚠️ Need .NET client | ⭐⭐⭐ Long-term |
| **4. Protocol Buffers Endpoint** | High (3-4w) | Medium | ✅ Complete | ✅ Yes | ⭐⭐⭐⭐ Best compromise |
| **5. JSON File Bridge** | Medium (1w) | Low | ✅ Complete | ✅ Yes | ⭐⭐ Prototyping |

---

## Use Case Analysis

### When Live Telemetry Makes Sense

✅ **Use Live Telemetry if:**

1. **Existing .NET Games/Tools**
   - Already using Live Telemetry in production
   - .NET applications that need metrics collection
   - Integration with Ubisoft Live Telemetry infrastructure is required

2. **Ubisoft Infrastructure Preference**
   - Want managed infrastructure (no self-hosting)
   - Need multi-environment support (dev, staging, prod)
   - Require UbiServices authentication

3. **Simple Metrics Collection**
   - Only need numeric measurements (long/double)
   - Don't need logs or traces
   - Metrics-focused use case

4. **VictoriaMetrics Backend**
   - Prefer VictoriaMetrics for metrics storage
   - PromQL for queries
   - Time-series database benefits

### When Micromegas Makes Sense

✅ **Use Micromegas if:**

1. **Unified Observability**
   - Need logs, metrics, AND traces together
   - Want to correlate across telemetry types
   - Single query interface for all observability data

2. **High-Frequency Events**
   - Need to capture 100k+ events/second
   - Low overhead critical (20ns per event)
   - Span instrumentation required

3. **Multi-Language Support**
   - Rust, Python, TypeScript applications
   - Need consistent instrumentation across languages
   - Future .NET support planned

4. **Self-Hosted / Cloud-Agnostic**
   - Want to self-host infrastructure
   - Cloud deployment flexibility (S3, GCS, etc.)
   - Open-source preference

5. **Data Lake / Analytics**
   - Need Parquet format for data lake
   - Want to use DataFusion for complex queries
   - Arrow-based analytics workflows

### When to Integrate Both

⚠️ **Consider integration if:**

1. **Transitional Period**
   - Migrating from Live Telemetry to Micromegas
   - Need to support both during transition
   - Gradual rollout to teams

2. **.NET Applications with Micromegas Backend**
   - Have .NET apps that need telemetry
   - Want to store in Micromegas for unified view
   - Willing to build bridge or Protocol Buffers endpoint

3. **Cross-Team Collaboration**
   - Some teams use Live Telemetry (games)
   - Other teams use Micromegas (services)
   - Need unified analytics across teams

---

## Architectural Recommendations

### Primary Recommendation: Keep Systems Separate (Short-Term)

**Rationale:**
1. Different use cases (game metrics vs service observability)
2. Different technology stacks (.NET vs Rust/Python/TypeScript)
3. Integration complexity outweighs benefits
4. Both systems work well independently

**Approach:**
- Use Live Telemetry for .NET games and applications
- Use Micromegas for Rust/Python/TypeScript services
- Accept fragmented observability for now

**Next Steps:**
1. Document which applications use which system
2. Evaluate if unified observability is worth the effort
3. Plan long-term strategy (see below)

---

### Secondary Recommendation: Protocol Buffers Endpoint (Medium-Term)

**If unified observability becomes priority:**

**Approach:**
Add Protocol Buffers ingestion endpoint to Micromegas telemetry-ingestion-srv

**Benefits:**
- ✅ .NET applications can use existing Live Telemetry SDK
- ✅ All data stored in Micromegas (unified storage)
- ✅ No need to rewrite .NET client immediately
- ✅ Gradual migration path

**Implementation:**
1. Add Protocol Buffers schema to Micromegas
2. Implement decoder in `telemetry-ingestion-srv` (Rust)
3. Map Live Telemetry instruments to Micromegas events
4. Configure Live Telemetry to use custom endpoint

**Effort:** 3-4 weeks

---

### Long-Term Recommendation: Micromegas .NET Client

**For complete unification:**

**Approach:**
Build native Micromegas client library for .NET (similar to Rust client)

**Benefits:**
- ✅ Consistent instrumentation across all languages
- ✅ Unified data model (no format conversion)
- ✅ Best performance (direct Arrow encoding)
- ✅ Single observability platform

**Implementation:**
1. Design .NET API (similar to Rust `micromegas-tracing`)
2. Implement Arrow encoding in C#
3. HTTP sink for ingestion
4. NuGet package distribution
5. Migration guide from Live Telemetry

**Effort:** 2-3 months

**When to do this:**
- If .NET applications become major part of ecosystem
- If unified observability is critical business requirement
- If resources available for .NET client development

---

## Technical Feasibility

### Protocol Buffers Endpoint Implementation

**Complexity:** Medium-High

**Required Work:**
1. **Import Protocol Buffers Schema** (2-3 days)
   - Copy `.proto` files from Live Telemetry
   - Generate Rust bindings with `prost`
   - Validate schema compatibility

2. **Implement Decoder** (1-2 weeks)
   ```rust
   // Pseudocode
   pub async fn ingest_live_telemetry(
       body: Bytes,
       headers: HeaderMap,
   ) -> Result<()> {
       // Decode Protocol Buffers
       let message = LiveTelemetryMessage::decode(&body)?;

       // Extract instruments and measurements
       let instruments = extract_instruments(&message)?;

       // Convert to Micromegas events
       let events = instruments_to_events(instruments)?;

       // Store in PostgreSQL + object storage
       store_events(events).await?;

       Ok(())
   }
   ```

3. **Format Transformation** (1 week)
   - Map Live Telemetry instruments to Micromegas spans/events
   - Handle timestamp conversion
   - Preserve measurement types (long, double, string)

4. **Testing** (3-5 days)
   - Unit tests for decoder
   - Integration tests with Live Telemetry client
   - Performance testing

**Risk Factors:**
- Schema evolution (Protocol Buffers v1 vs latest)
- Timestamp system differences
- Semantic mapping (instruments → events)
- Performance impact on ingestion server

---

### Micromegas .NET Client Implementation

**Complexity:** High

**Required Work:**
1. **Client Library Design** (1 week)
   - API design (similar to `micromegas-tracing`)
   - Span/event/metric abstractions
   - Attribute handling

2. **Arrow Encoding** (2-3 weeks)
   - Use `Apache.Arrow` NuGet package
   - Implement `RecordBatch` builders
   - Parquet file generation

3. **HTTP Sink** (1 week)
   - HTTP client for ingestion endpoint
   - Retry policies
   - Buffering and batching

4. **Instrumentation Helpers** (1-2 weeks)
   - Attributes (similar to Rust macros)
   - Async context propagation
   - Structured logging integration

5. **NuGet Package** (2-3 days)
   - Package configuration
   - Documentation
   - Publish to Artifactory

6. **Testing & Examples** (1-2 weeks)
   - Unit tests
   - Integration tests
   - Sample applications

**Total Effort:** 2-3 months

**Risk Factors:**
- Arrow library maturity in .NET
- Performance compared to Protocol Buffers + Snappy
- API ergonomics for .NET developers
- Maintenance burden (another client to support)

---

## Cost-Benefit Analysis

| Approach | Cost | Benefit | ROI | Timeline |
|----------|------|---------|-----|----------|
| **Keep Separate** | Very Low (status quo) | Low (fragmented) | N/A | Immediate |
| **VictoriaMetrics Bridge** | High (2-3w + infra) | Medium (unified storage) | ⭐⭐ Low | 3-4 weeks |
| **Dual System** | Low (status quo) | Medium (specialized) | ⭐⭐⭐ Medium | Immediate |
| **Protocol Buffers Endpoint** | High (3-4w) | High (unified + .NET support) | ⭐⭐⭐⭐ High | 4-5 weeks |
| **JSON File Bridge** | Medium (1w) | Low (prototype only) | ⭐ Very Low | 1-2 weeks |
| **Micromegas .NET Client** | Very High (2-3m) | Very High (complete unification) | ⭐⭐⭐⭐⭐ Very High | 3-4 months |

---

## Integration Checklist

### Before Integration

- [ ] Identify .NET applications currently using Live Telemetry
- [ ] Assess volume of telemetry data from these applications
- [ ] Evaluate business value of unified observability
- [ ] Determine if .NET support is long-term requirement
- [ ] Check if Ubisoft Live Telemetry infrastructure is still required

### For Protocol Buffers Endpoint

- [ ] Obtain Live Telemetry Protocol Buffers schema
- [ ] Add `prost` and related dependencies to `telemetry-ingestion-srv`
- [ ] Implement decoder and transformation logic
- [ ] Add new HTTP endpoint `/api/live-telemetry/ingest`
- [ ] Test with Live Telemetry client using custom endpoint
- [ ] Document configuration for .NET apps
- [ ] Monitor performance impact on ingestion server

### For .NET Client

- [ ] Design .NET API (review with team)
- [ ] Set up .NET project structure
- [ ] Implement Arrow encoding
- [ ] Implement HTTP sink
- [ ] Create example applications
- [ ] Write documentation
- [ ] Publish to NuGet / Artifactory
- [ ] Create migration guide from Live Telemetry

---

## Conclusion

### Summary

**Live Telemetry Client** is a well-designed .NET library for game metrics collection, integrated with Ubisoft's managed infrastructure. It uses Protocol Buffers and Snappy compression for efficient transport to VictoriaMetrics backend.

**Micromegas** is a unified observability platform built in Rust with high-performance instrumentation, supporting logs, metrics, and traces with Arrow/Parquet storage and DataFusion queries.

**Key Differences:**
- **Use Case**: Live Telemetry = game metrics | Micromegas = unified observability
- **Language**: Live Telemetry = .NET only | Micromegas = Rust/Python/TypeScript
- **Data Model**: Live Telemetry = Protocol Buffers | Micromegas = Apache Arrow
- **Storage**: Live Telemetry = VictoriaMetrics | Micromegas = PostgreSQL + Parquet
- **Infrastructure**: Live Telemetry = Ubisoft-managed | Micromegas = self-hosted

### Recommendations by Scenario

#### If .NET Applications Are Not a Priority
**→ Keep Systems Separate**
- Use Live Telemetry for existing .NET games
- Use Micromegas for services (Rust/Python/TypeScript)
- Accept fragmented observability
- **Effort:** None (status quo)

#### If .NET Support Is Important Short-Term
**→ Implement Protocol Buffers Endpoint**
- Add Live Telemetry format ingestion to Micromegas
- .NET apps use existing Live Telemetry SDK
- Unified storage in Micromegas
- **Effort:** 3-4 weeks

#### If Unified Observability Is Critical Long-Term
**→ Build Micromegas .NET Client**
- Native .NET client for Micromegas
- Complete unification across all languages
- Best long-term solution
- **Effort:** 2-3 months

#### For Quick Prototyping
**→ JSON File Bridge**
- Export Live Telemetry to JSON
- Parse and forward to Micromegas
- Good for testing integration
- **Effort:** 1 week

### Final Recommendation

**Primary: Keep systems separate for now** unless there's a specific business requirement for unified observability across .NET and Rust/Python/TypeScript applications.

**If integration is needed:**
1. Start with **Protocol Buffers endpoint** (medium-term)
2. Plan **Micromegas .NET client** (long-term)
3. Avoid complex bridge solutions (VictoriaMetrics bridge)

**Key Decision Point:**
- If .NET is a small part of ecosystem → Stay separate
- If .NET is significant and unified observability is valuable → Invest in integration

---

## References

### Live Telemetry Documentation
- **Repository**: https://gitlab-ncsa.ubisoft.org/tgdp/telemetry/client/LiveTelemetry.Client
- **Teams Channel**: [Live Telemetry Team](https://teams.microsoft.com/l/channel/19%3Af427262de9ed4a758fc7db821aaeb118%40thread.tacv2/13.8%20-%20telemetry)
- **Local Debugging**: https://backstage.qf.ubisoft.com/docs/default/system/telemetry/user-guide/help_to_debug/

### Micromegas Documentation
- **Architecture**: `CLAUDE.md`, `AI_GUIDELINES.md` in repository
- **Rust Client**: `rust/public/` crate
- **Python Client**: `python/micromegas/` directory

### Related Evaluations
- `MAP_ARCHITECTURE_EVALUATION.md` - Technology stack comparison for map visualization
- `ATLAS_MTS_UBIMAPS_EVALUATION.md` - Atlas integration evaluation

### Technical References
- **Protocol Buffers**: https://protobuf.dev/
- **Apache Arrow**: https://arrow.apache.org/
- **Snappy Compression**: https://github.com/google/snappy
- **VictoriaMetrics**: https://victoriametrics.com/
