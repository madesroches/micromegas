# Properties Writing Optimization Plan for Log Entries and Measures Views

## Architecture Refactoring Required

### ProcessInfo Separation Strategy

Current `ProcessInfo` serves two distinct use cases:
1. **Instrumentation**: Used by `tracing`, `telemetry-sink`, HTTP transmission (CBOR serialization)
2. **Analytics**: Used by analytics engine for database queries and Arrow/Parquet generation

**Problem**: Cannot require instrumented applications to send properties in binary JSONB format.

**Solution**: Create separate analytics-optimized struct while maintaining compatibility.

### New Structure Design

```rust
// In analytics/src/metadata.rs
pub type SharedJsonbSerialized = Arc<Vec<u8>>;

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    // Core fields (same as ProcessInfo)
    pub process_id: uuid::Uuid,
    pub exe: String,
    pub username: String,
    pub realname: String,
    pub computer: String,
    pub distro: String,
    pub cpu_brand: String,
    pub tsc_frequency: i64,
    pub start_time: chrono::DateTime<chrono::Utc>,
    pub start_ticks: i64,
    pub parent_process_id: Option<uuid::Uuid>,

    // Analytics-optimized fields
    pub properties: SharedJsonbSerialized,            // Pre-serialized JSONB properties
}

// Note: From<ProcessInfo> conversion may not be needed -
// ProcessMetadata will primarily be created directly from database rows
// impl From<ProcessInfo> for ProcessMetadata { ... }
```

## Priority-Ordered Task List

### High Priority (Immediate CPU savings)

1. **Create ProcessMetadata struct**
   - Define new struct in `analytics/src/metadata.rs`
   - Add `SharedJsonbSerialized` type alias for `Arc<Vec<u8>>`
   - Add `properties: SharedJsonbSerialized` field for pre-serialized JSONB
   - Eliminate the need for separate HashMap storage in analytics layer
   - Skip `From<ProcessInfo>` conversion unless actually needed

2. **Update process_from_row() to pre-serialize JSONB**
   - Modify `process_from_row()` to return `ProcessMetadata`
   - Serialize JSONB once during database deserialization
   - Store serialized JSONB in `properties` field

3. **PropertySet pointer-based deduplication**
   - Use `Arc<Object>::as_ptr()` as cache key for PropertySets
   - Cache dictionary indices per unique pointer
   - Eliminate content hashing overhead

4. **Update analytics data structures to use ProcessMetadata**
   - Modify `LogEntry`, `MeasureRow`, and related structs to use `Arc<ProcessMetadata>`
   - Update `PartitionSourceBlock` and related analytics structures
   - Ensure backward compatibility with existing ProcessInfo in instrumentation layer

5. **Optimize property serialization in LogEntriesRecordBuilder**
   - Use pre-serialized `properties` directly from ProcessMetadata
   - Add pointer-based caching using `Arc::as_ptr()` for process properties
   - Avoid all re-serialization of process properties

### Medium Priority (Batch optimizations)

4. **Bulk dictionary building**
   - Collect unique property sets during block iteration
   - Serialize all unique sets in batch
   - Pre-allocate dictionary with computed indices

5. **Property set caching in block processors**
   - Add PropertySetCache to LogBlockProcessor and MetricsBlockProcessor
   - Cache serialized JSONB per unique PropertySet/ProcessMetadata pointer
   - Reuse cached results within partition

### Lower Priority (Advanced optimizations)

6. **Cross-block property interning**
   - Maintain global property set intern pool per view update
   - Reference counting for memory management

7. **Zero-copy JSONB handling**
   - Direct serialization into dictionary builder buffers
   - Eliminate intermediate Vec<u8> allocations

## Migration Strategy

### Phase 1: Create Analytics Infrastructure
1. Create `ProcessMetadata` struct with pre-serialized JSONB support
2. Create helper functions for JSONB serialization
3. Add conversion functions only if actually needed (avoid eager implementation)

### Phase 2: Update Database Layer
1. Modify `process_from_row()` to return `ProcessMetadata`
2. Update all analytics queries to use new struct
3. Ensure all database->analytics conversions pre-serialize JSONB

### Phase 3: Update Analytics Data Structures
1. Replace `Arc<ProcessInfo>` with `Arc<ProcessMetadata>` in:
   - `LogEntry` struct
   - `MeasureRow` struct
   - `PartitionSourceBlock` struct
   - All analytics pipeline components

### Phase 4: Optimize Property Writing
1. Update `LogEntriesRecordBuilder` and `MetricsRecordBuilder` to use pre-serialized JSONB
2. Implement pointer-based caching for process properties
3. Add PropertySet pointer-based deduplication

### Phase 5: Advanced Optimizations
1. Implement bulk dictionary building
2. Add cross-block property interning
3. Zero-copy JSONB optimizations

## Current CPU Usage Issues

- Properties parsed from DB: `micromegas_property[]` → `Vec<Property>` → `HashMap` → JSONB
- Same process properties serialized repeatedly per log entry/measure
- PropertySets use `Arc<Object>` but we don't leverage pointer equality
- Per-row JSONB serialization instead of batching
- **Key Issue**: ProcessInfo serves both instrumentation and analytics but can't require binary JSONB in instrumentation layer

## Compatibility Requirements

- **Instrumentation layer**: Must continue using `ProcessInfo` with `HashMap<String, String>` properties
- **Analytics layer**: Can use optimized `ProcessMetadata` with pre-serialized JSONB properties
- **Wire protocol**: HTTP/CBOR transmission uses original ProcessInfo format
- **Database**: Store properties as `micromegas_property[]`, convert to analytics format on read

## Success Criteria

- 30-50% reduction in CPU cycles for property writing (high-duplication scenarios)
- 15-25% reduction in CPU usage for overall block processing
- 20-40% reduction in allocation overhead
- Zero data corruption, backward compatibility maintained
- Clean separation between instrumentation and analytics concerns