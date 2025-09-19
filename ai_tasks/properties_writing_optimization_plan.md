# Properties Writing Optimization Plan for Log Entries and Measures Views

## Priority-Ordered Task List

### High Priority (Immediate CPU savings)

1. **Pre-serialize JSONB buffers in ProcessInfo**
   - Add `properties_jsonb: Option<Arc<Vec<u8>>>` field to ProcessInfo
   - Serialize JSONB once in `process_from_row()` instead of per-row
   - Use Arc for pointer-based deduplication

2. **PropertySet pointer-based deduplication**
   - Use `Arc<Object>::as_ptr()` as cache key for PropertySets
   - Cache dictionary indices per unique pointer
   - Eliminate content hashing overhead

3. **Process properties Arc wrapping**
   - Change ProcessInfo properties from `HashMap<String, String>` to `Arc<HashMap<String, String>>`
   - Enable pointer equality for process property deduplication

### Medium Priority (Batch optimizations)

4. **Bulk dictionary building**
   - Collect unique property sets during block iteration
   - Serialize all unique sets in batch
   - Pre-allocate dictionary with computed indices

5. **Property set caching in block processors**
   - Add PropertySetCache to LogBlockProcessor and MetricsBlockProcessor
   - Cache serialized JSONB per unique PropertySet/ProcessInfo pointer
   - Reuse cached results within partition

### Lower Priority (Advanced optimizations)

6. **Cross-block property interning**
   - Maintain global property set intern pool per view update
   - Reference counting for memory management

7. **Zero-copy JSONB handling**
   - Direct serialization into dictionary builder buffers
   - Eliminate intermediate Vec<u8> allocations

## Current CPU Usage Issues

- Properties parsed from DB: `micromegas_property[]` → `Vec<Property>` → `HashMap` → JSONB
- Same process properties serialized repeatedly per log entry/measure
- PropertySets use `Arc<Object>` but we don't leverage pointer equality
- Per-row JSONB serialization instead of batching

## Success Criteria

- 30-50% reduction in CPU cycles for property writing (high-duplication scenarios)
- 15-25% reduction in CPU usage for overall block processing
- 20-40% reduction in allocation overhead
- Zero data corruption, backward compatibility maintained