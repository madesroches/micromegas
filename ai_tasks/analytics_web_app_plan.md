# Analytics Web App Implementation Plan (Phase 1)

## Overview

Create a modern, production-ready analytics web application for generating and downloading Perfetto traces from micromegas telemetry data. This serves as the foundation for testing async span implementation and provides immediate value as a standalone analytics tool.

## Architecture Decision

**Hybrid architecture combining Rust backend (Axum) with modern frontend framework**

### Backend: Rust + Axum (Existing Infrastructure)
- **Framework**: Axum 0.8 (already in use across micromegas services)
- **Benefits**: Type safety, performance, seamless integration with existing FlightSQL clients
- **Leverages**: Existing `perfetto_trace_client.rs`, analytics connections, observability middleware

### Frontend Framework Decision: Next.js 15 (React)

**Selected**: Next.js 15 with React 18 for optimal implementation quality

**Why Next.js/React for This Project**:
- **Complex Data Visualization**: Perfetto traces require sophisticated UI components (timelines, process trees, progress tracking)
- **Mature Ecosystem**: Extensive libraries for charts, tables, file handling, streaming
- **TypeScript Excellence**: Superior React + TypeScript patterns and tooling
- **Server Components**: React Server Components ideal for process metadata rendering
- **HTTP Streaming**: Built-in support for streaming responses (perfect for progress updates and large trace files)
- **Developer Productivity**: Faster development with established patterns

**Alternative Options Considered**:
- **SvelteKit**: Excellent performance but less ecosystem maturity for complex data visualization
- **Nuxt 3 (Vue)**: Good developer experience but React ecosystem better for this use case
- **WebAssembly (WASM)**: Analyzed full WASM UI and hybrid approaches - see detailed analysis below

## Recommended Technology Stack (Next.js + Axum)

### Backend (Rust):
```rust
// Dependencies to add to workspace Cargo.toml
serde_json = "1.0"
serde = { version = "1.0", features = ["derive"] }
tower-http = { version = "0.5", features = ["fs", "cors", "trace"] }
axum = { version = "0.8", features = ["json", "query"] }
```

### Frontend (Next.js 15):
```json
{
  "name": "perfetto-web-ui",
  "dependencies": {
    "next": "^15.0.0",
    "react": "^18.3.0",
    "react-dom": "^18.3.0",
    "@tanstack/react-query": "^5.8.0",
    "recharts": "^2.8.0",
    "react-table": "^7.8.0",
    "@radix-ui/react-select": "^2.0.0",
    "@radix-ui/react-progress": "^1.0.0",
    "lucide-react": "^0.292.0",
    "tailwindcss": "^3.3.0",
    "class-variance-authority": "^0.7.0"
  },
  "devDependencies": {
    "typescript": "^5.4.0",
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "eslint": "^8.57.0",
    "eslint-config-next": "15.0.0"
  }
}
```

## Detailed Implementation Tasks

### 1. Backend API Server (`rust/perfetto-web-srv/`)

**REST API Endpoints**:
- `GET /api/processes` - List available processes with metadata
- `GET /api/perfetto/{process_id}/info` - JSON metadata (size, generation time, span counts)
- `POST /api/perfetto/{process_id}/validate` - Validate trace structure
- `GET /api/health` - Service health check

**HTTP Streaming Endpoints** (for progress + download):
- `POST /api/perfetto/{process_id}/generate` - Generate trace with streaming progress + binary data
- Stream format: 
  - Progress chunks: `{"type": "progress", "percentage": 25, "message": "Processing spans..."}`
  - Final binary chunk: Raw perfetto trace bytes (application/octet-stream)
- Single request handles both progress updates and final trace delivery

**Static File Serving**:
- Serve Next.js build artifacts from `/.next/static/`
- Handle dynamic routing and API routes

### 2. Modern React UI Components

**Process Selection Interface**:
```typescript
// Components to implement:
- ProcessTable: Server Component with Tanstack Table for filtering/sorting
- ProcessCard: Interactive cards with metadata, generation status
- TimeRangePicker: Custom timeline component with zoom/pan
- ProcessSearch: Client-side search with debounced filtering
- ProcessList: Refreshable process list with manual/auto-refresh options
```

**Trace Generation Interface with HTTP Streaming**:
```typescript
// React Components:
- TraceGenerationForm: Form with span type selection, time ranges
- StreamingProgressIndicator: Real-time progress via HTTP streaming
- DownloadQueue: Manage multiple concurrent streaming generations
- SpanTypeSelector: Toggle switches using Radix UI Switch
- TraceValidator: Upload and validate trace files client-side

// Streaming Implementation:
const generateTrace = async (processId: string, options: GenerateOptions) => {
  const response = await fetch(`/api/perfetto/${processId}/generate`, {
    method: 'POST',
    body: JSON.stringify(options)
  });
  
  const reader = response.body.getReader();
  const chunks = [];
  let progressComplete = false;
  
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    
    if (!progressComplete) {
      // Try to parse as JSON progress update
      try {
        const chunk = new TextDecoder().decode(value);
        const update = JSON.parse(chunk);
        
        if (update.type === 'progress') {
          setProgress(update.percentage);
          setMessage(update.message);
          continue;
        } else if (update.type === 'binary_start') {
          progressComplete = true;
          continue;
        }
      } catch {
        // Not JSON, must be binary data
        progressComplete = true;
      }
    }
    
    // Collect binary chunks
    chunks.push(value);
  }
  
  // Create blob and download
  const blob = new Blob(chunks, { type: 'application/octet-stream' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `perfetto-${processId}-${Date.now()}.pb`;
  a.click();
  URL.revokeObjectURL(url);
};
```

**Advanced React Features**:
```typescript
// Advanced Components:
- TraceComparison: Side-by-side comparison with diff highlighting
- BulkOperations: Batch select/generate with parallel processing
- ExportDialog: Modal with format options, compression settings
- ErrorBoundary: Comprehensive error handling with retry mechanisms
- NotificationSystem: Toast notifications for operations
```

### 3. Production-Ready Features

**Performance Optimizations**:
- **Response Streaming**: Stream large traces to prevent memory issues
- **Compression**: Gzip/Brotli compression for web assets
- **Caching**: Redis/in-memory cache for process metadata
- **Rate Limiting**: Prevent abuse of trace generation endpoints

**Observability**:
- **Metrics**: Request counts, generation times, error rates
- **Tracing**: Distributed tracing for all operations
- **Health Checks**: Kubernetes-ready liveness/readiness probes
- **Logging**: Structured logging with correlation IDs

**Security**:
- **CORS Configuration**: Proper cross-origin resource sharing
- **Authentication**: JWT/session-based auth (extensible)
- **Input Validation**: Comprehensive request validation
- **Rate Limiting**: Per-IP and per-user rate limiting

**Deployment**:
- **Docker Support**: Multi-stage builds with distroless images
- **Kubernetes Manifests**: Ready-to-deploy K8s resources
- **Environment Configuration**: 12-factor app configuration
- **Graceful Shutdown**: Handle SIGTERM properly

### 4. Development Experience

**Hot Reloading & Development Server**: 
```bash
# Frontend development
cd frontend && npm run dev  # Next.js dev server with Fast Refresh

# Backend development  
cd rust && cargo watch -x "run --bin perfetto-web-srv"

# Full stack development
npm run dev:full-stack  # Runs both with proper proxy
```

**Type Safety & Code Generation**:
```typescript
// Generate TypeScript types from Rust structs
use ts-rs or similar for automatic type generation

// Example shared types:
interface ProcessInfo {
  id: string;
  name: string;
  pid: number;
  start_time: number;
  end_time: number;
  span_counts: SpanCounts;
}
```

**Testing Strategy**:
```typescript
// Testing Stack:
- Jest + React Testing Library: Component unit tests
- MSW (Mock Service Worker): API mocking
- Playwright: E2E tests
- Vitest: Fast unit tests for utilities

// Rust backend:
- cargo test: Unit and integration tests
- wiremock: Mock external services
```

### 5. Scalability Considerations

**Horizontal Scaling**:
- Stateless design for easy horizontal scaling
- Load balancer friendly (health checks, graceful shutdown)
- Session storage externalization ready

**Vertical Scaling**:
- Efficient memory usage with streaming
- Async/await throughout for non-blocking operations
- Connection pooling for FlightSQL clients

**Monitoring**:
- Prometheus metrics endpoint
- OpenTelemetry tracing integration
- Custom dashboards for trace generation metrics

## Benefits of Next.js/React Approach

- **Implementation Quality**: Leveraging deep React expertise for better code quality
- **Rich Ecosystem**: Mature libraries for complex data visualization needs
- **TypeScript Excellence**: Superior React + TypeScript patterns and tooling
- **Server Components**: Optimal performance for process metadata rendering
- **Streaming Support**: Built-in streaming ideal for large trace file downloads
- **Production Ready**: Battle-tested stack deployed at scale everywhere
- **Developer Productivity**: Faster development with established patterns
- **Component Reusability**: Modular components for future feature expansion
- **Testing Maturity**: Comprehensive testing ecosystem and best practices

## WebAssembly (WASM) Decision Analysis

**WASM Options Evaluated**:

**Option 1: Full WASM UI** (Yew, Leptos, Dioxus)
- ✅ **Pros**: End-to-end Rust types, code reuse, consistent performance
- ❌ **Cons**: Large bundles (800KB-1.5MB), limited ecosystem, debugging complexity

**Option 2: Hybrid React + WASM Modules**
- ✅ **Pros**: React ecosystem + WASM for computations, selective optimization
- ❌ **Cons**: Dual build complexity, limited benefits (processing is server-side)

**Option 3: Next.js/React (Selected)**
- ✅ **Pros**: Mature ecosystem, proven patterns, optimal for UI-heavy workloads
- ✅ **Workload Match**: Heavy processing on server, light UI interactions on client

**Decision Rationale**:
1. **Workload Analysis**: Computational work (trace generation, data querying) happens server-side; client handles UI rendering and HTTP streaming where JavaScript excels
2. **Bundle Efficiency**: Next.js (~200-300KB) vs WASM frameworks (~800KB-1.5MB base)
3. **Development Speed**: React expertise enables faster, higher-quality implementation
4. **Ecosystem Maturity**: Rich visualization libraries, testing tools, streaming support, component systems

**Future WASM Considerations**:
WASM modules could be valuable for future enhancements:
- Client-side trace file validation (large file processing)
- Advanced trace analysis calculations in browser
- Offline mode capabilities
- Real-time filtering of large datasets

**Implementation Strategy**: Start with Next.js/React foundation, add WASM modules selectively when computational benefits justify the complexity.

## HTTP Streaming Progress Updates

**Why HTTP Streaming over Alternatives**:

**HTTP Streaming (Selected)**:
- ✅ **Idiomatic**: Standard pattern for progress reporting during long operations (GitHub Actions, Docker, npm)
- ✅ **Simple Infrastructure**: Standard HTTP, no connection state management
- ✅ **Natural Fit**: Progress reporting during file generation is exactly what HTTP streaming excels at
- ✅ **Request-Response Model**: Each trace generation is a discrete operation
- ✅ **Error Handling**: Standard HTTP retry/error mechanisms

**Alternatives Considered**:
- **WebSockets**: Overkill for unidirectional progress updates, adds connection complexity
- **Polling**: Inefficient, delayed updates, server resource waste
- **Server-Sent Events**: Good alternative but HTTP streaming simpler for this use case

**Streaming Implementation Pattern**:
```rust
// Backend: Stream JSON progress updates followed by binary data
async fn generate_perfetto_trace() -> impl IntoResponse {
    let (tx, rx) = tokio::sync::mpsc::channel(100);
    
    tokio::spawn(async move {
        // Send progress updates as JSON
        tx.send(Bytes::from(serde_json::to_string(&json!({
            "type": "progress", 
            "percentage": 10, 
            "message": "Querying process metadata"
        })).unwrap())).await;
        
        tx.send(Bytes::from(serde_json::to_string(&json!({
            "type": "progress", 
            "percentage": 30, 
            "message": "Processing thread spans"
        })).unwrap())).await;
        
        tx.send(Bytes::from(serde_json::to_string(&json!({
            "type": "progress", 
            "percentage": 60, 
            "message": "Processing async spans"
        })).unwrap())).await;
        
        tx.send(Bytes::from(serde_json::to_string(&json!({
            "type": "progress", 
            "percentage": 90, 
            "message": "Finalizing trace file"
        })).unwrap())).await;
        
        // Signal transition to binary data
        tx.send(Bytes::from(serde_json::to_string(&json!({
            "type": "binary_start"
        })).unwrap())).await;
        
        // Generate and stream the actual trace file
        let trace_data = generate_perfetto_trace_data().await;
        
        // Send binary data in chunks
        for chunk in trace_data.chunks(8192) {
            tx.send(Bytes::from(chunk.to_vec())).await;
        }
    });
    
    Body::from_stream(ReceiverStream::new(rx))
}
```

**Benefits**:
- **Real-time Feedback**: Immediate progress updates without polling overhead
- **Single Request**: No temporary file storage, everything streamed in one HTTP request
- **Memory Efficient**: Binary data streamed directly without server-side storage
- **Standard HTTP**: Works with all HTTP infrastructure (proxies, load balancers, CDNs)
- **Error Recovery**: Standard HTTP error handling and retry mechanisms
- **Logging/Monitoring**: Standard HTTP request/response logging and metrics

## Expected Outcomes

1. **Production-Ready Web Interface**: Immediately usable tool for generating Perfetto traces
2. **Testing Foundation**: Platform for validating all subsequent async span implementation phases
3. **User Experience**: Modern, responsive UI with real-time progress feedback
4. **Scalable Architecture**: Ready for production deployment and future feature additions
5. **Developer Productivity**: Fast development cycle with hot reloading and comprehensive testing