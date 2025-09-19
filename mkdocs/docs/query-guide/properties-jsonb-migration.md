# Properties to JSONB Migration

This guide explains the migration from legacy properties format to the new dictionary-encoded JSONB format in Micromegas, providing improved storage efficiency and query performance.

## Overview

Micromegas has migrated properties storage from the original `List<Struct<key: String, value: String>>` format to an optimized `Dictionary<Int32, Binary>` JSONB format. This migration provides:

- **70-80% storage reduction** through dictionary compression
- **Improved query performance** with native JSONB operations
- **Full backward compatibility** - all existing queries work unchanged
- **Zero downtime** deployment via automatic schema versioning

## Migration Status

✅ **Migration Complete** - All view sets now use the new JSONB format:

| View Set | Status | Schema Version | Properties Fields |
|----------|--------|----------------|-------------------|
| **blocks** | ✅ Complete | v2 | `processes.properties`, `streams.properties` |
| **processes** | ✅ Complete | Inherited | `properties` |
| **streams** | ✅ Complete | Inherited | `properties` |
| **log_entries** | ✅ Complete | v5 | `properties`, `process_properties` |
| **measures** | ✅ Complete | v5 | `properties`, `process_properties` |

## Key Benefits

### Storage Efficiency
- Dictionary compression eliminates redundant JSONB objects
- Typical storage reduction of 70-80% for property data
- Better memory utilization during query processing

### Performance Improvements
- Native JSONB operations via `property_get()` and `properties_length()`
- Optimized UDF implementations handle both legacy and JSONB formats
- Automatic pass-through optimization for JSONB data

### Operational Benefits
- Zero-downtime migration through schema versioning
- Automatic partition rebuilds triggered by schema hash changes
- Full backward compatibility with existing SQL queries

## Technical Details

### Storage Format Comparison

**Legacy Format (Before Migration):**
```sql
-- Inefficient nested structure
List<Struct<
    key: Utf8,
    value: Utf8
>>
```

**New Format (After Migration):**
```sql
-- Optimized dictionary-encoded JSONB
Dictionary<Int32, Binary>
```

### Migration Method

1. **Read-time Transformation**: Existing data converted to JSONB during query processing
2. **Schema Versioning**: Version increments trigger automatic partition rebuilds
3. **Zero Database Changes**: PostgreSQL schema remains unchanged
4. **Automatic Inheritance**: Process/stream views inherit JSONB from blocks table

## Query Compatibility

### No Changes Required

All existing queries continue to work unchanged:

```sql
-- These queries work identically before and after migration
SELECT property_get(properties, 'service') as service_name
FROM log_entries
WHERE properties_length(properties) > 0;

SELECT property_get(process_properties, 'thread-name') as thread_name
FROM measures
WHERE property_get(process_properties, 'version') = '1.2.3';
```

### Enhanced Functions

The migration enhances existing functions with automatic format detection:

#### `property_get(properties, key)`
- **Automatically handles** all property formats (legacy, JSONB, dictionary-encoded)
- **Returns** same results regardless of underlying storage format
- **Performance optimized** for JSONB with pass-through operations

#### `properties_length(properties)`
- **Works with** all property formats transparently
- **Replaces** legacy `array_length(properties)` calls
- **Consistent behavior** across all view sets

## Migration Process

### Automatic Schema Versioning

The migration uses schema versioning to trigger automatic rebuilds:

1. **Schema Hash Changes**: Property format changes increment schema version
2. **Partition Rebuilds**: New schema versions automatically rebuild affected partitions
3. **Transparent Process**: No manual intervention required
4. **Rollback Safety**: Previous partitions remain until rebuild completion

### View Set Details

#### Phase 1: Core Infrastructure
- **blocks table**: Updated to output JSONB via `PropertiesColumnReader`
- **Schema version**: v1 → v2
- **Impact**: Foundation for all other view sets

#### Phase 2: Inherited Views
- **processes/streams**: Automatically inherit JSONB from blocks table
- **No explicit changes**: SQL inheritance provides automatic JSONB format
- **Zero effort**: Benefits gained without code modifications

#### Phase 3: Direct Schema Updates
- **log_entries/measures**: Direct schema and builder updates
- **Schema version**: v4 → v5
- **Arrow builders**: Updated to use JSONB dictionary builders

## Best Practices

### Query Optimization

Use the new format for maximum performance:

```sql
-- Optimal: Direct property access (post-migration)
SELECT property_get(properties, 'hostname') as hostname
FROM log_entries
WHERE properties_length(properties) > 0;

-- Still works: Legacy compatibility maintained
SELECT property_get(properties, 'hostname') as hostname
FROM log_entries
WHERE property_get(properties, 'env') = 'production';
```

### Property Filtering

Take advantage of JSONB efficiency:

```sql
-- Efficient property-based filtering
SELECT time, level, msg
FROM log_entries
WHERE property_get(properties, 'service') = 'web_server'
  AND property_get(properties, 'env') = 'production'
  AND time >= NOW() - INTERVAL '1 hour';
```

### Aggregation Patterns

Properties work efficiently in GROUP BY operations:

```sql
-- Service-level error analysis
SELECT
    property_get(properties, 'service') as service,
    COUNT(*) as total_logs,
    COUNT(CASE WHEN level <= 2 THEN 1 END) as error_count
FROM log_entries
WHERE time >= NOW() - INTERVAL '1 day'
  AND properties_length(properties) > 0
GROUP BY service
ORDER BY error_count DESC;
```

## Monitoring and Validation

### Performance Verification

Monitor query performance improvements:

```sql
-- Property access patterns
SELECT
    COUNT(*) as properties_queries,
    AVG(properties_length(properties)) as avg_prop_count
FROM log_entries
WHERE time >= NOW() - INTERVAL '1 hour'
  AND properties_length(properties) > 0;
```

### Data Integrity Checks

Verify migration completion:

```sql
-- Check property data consistency
SELECT
    COUNT(*) as total_rows,
    COUNT(CASE WHEN properties_length(properties) > 0 THEN 1 END) as with_properties,
    COUNT(CASE WHEN property_get(properties, 'hostname') IS NOT NULL THEN 1 END) as with_hostname
FROM processes
WHERE insert_time >= NOW() - INTERVAL '1 day';
```

## Troubleshooting

### Common Issues

#### Property Access Returns NULL
**Symptom**: `property_get()` returns NULL for existing properties
**Solution**: Verify property key names - JSONB is case-sensitive

#### Performance Not Improved
**Symptom**: Queries still slow after migration
**Solution**: Ensure using `properties_length()` instead of `array_length()`

### Rollback Procedures

If issues arise, the migration can be rolled back:

1. **Schema Revert**: Restore previous schema versions via code rollback
2. **Partition Rebuild**: Automatic rebuild occurs with old schema hash
3. **Query Compatibility**: No SQL changes needed during rollback

## Support

### Updated Functions

All property functions maintain backward compatibility:

- ✅ `property_get()` - Works with all formats
- ✅ `properties_length()` - Replaces `array_length()`
- ✅ `properties_to_jsonb()` - Pass-through for JSONB data
- ✅ `properties_to_dict()` - Dictionary encoding support

### Documentation

- **[Schema Reference](schema-reference.md)** - Updated field types and formats
- **[Functions Reference](functions-reference.md)** - Property function documentation
- **[Query Patterns](query-patterns.md)** - Optimized query examples

## Conclusion

The properties to JSONB migration represents a significant improvement in Micromegas storage efficiency and query performance. With full backward compatibility and automatic deployment, users benefit from:

- **Immediate storage savings** of 70-80% for property data
- **Improved query performance** through optimized JSONB operations
- **Zero operational impact** via seamless migration process
- **Future-proof architecture** for advanced property operations

All existing queries continue to work unchanged while gaining the benefits of the new optimized storage format.