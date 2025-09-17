# Properties Column Analysis

## Tables with Properties Columns

### PostgreSQL Tables
1. **processes** table
   - Column: `properties micromegas_property[]`
   - Stores process-level properties as array of key-value pairs

2. **streams** table
   - Column: `properties micromegas_property[]`
   - Stores stream-level properties as array of key-value pairs

### Custom PostgreSQL Type
- Type: `micromegas_property`
- Definition: `(key TEXT, value TEXT)` 
- Array type: `micromegas_property[]`

## Current Storage Format

### In PostgreSQL
- Properties stored as PostgreSQL array type `micromegas_property[]`
- Each property is a composite type with `key` and `value` fields
- Both key and value are TEXT fields

### In Arrow/Parquet (Analytics)
- Properties represented as `List<Struct>`
- Struct fields:
  - `key`: DataType::Utf8 (not nullable)
  - `value`: DataType::Utf8 (not nullable)
- List wrapper allows multiple key-value pairs per record

### Arrow Schema Definition
```rust
DataType::List(Arc::new(Field::new(
    "Property",
    DataType::Struct(Fields::from(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("value", DataType::Utf8, false),
    ])),
    false,
)))
```

## Access Patterns

### Writing (Ingestion)
1. Properties collected during telemetry capture
2. Sent to ingestion service via HTTP
3. Stored in PostgreSQL as `micromegas_property[]`
4. Eventually materialized to Parquet files in object storage

### Reading (Analytics)
1. **SQL Arrow Bridge** (`sql_arrow_bridge.rs`)
   - `PropertiesColumnReader` reads from PostgreSQL
   - Converts `micromegas_property[]` to Arrow `List<Struct>`

2. **Lakehouse Processing**
   - `partition_source_data.rs`: Reads properties during partitioning
   - `jit_partitions.rs`: Processes properties in JIT materialization
   - Properties flow through as `GenericListArray<i32>` (List arrays)

3. **Table Builders**
   - `log_entries_table.rs`: Has `properties` and `process_properties` fields
   - `metrics_table.rs`: Has `properties` and `process_properties` fields
   - Both use `ListBuilder<StructBuilder>` for construction

4. **Query Processing**
   - UDFs like `property_get` extract specific properties
   - Recently added dictionary encoding support in `property_get` UDF
   - Properties can be filtered/queried in DataFusion SQL

## Key Files Using Properties

1. **Core Definition & Conversion**:
   - `rust/ingestion/src/sql_telemetry_db.rs` - Table definitions
   - `rust/analytics/src/sql_arrow_bridge.rs` - PostgreSQL to Arrow conversion
   - `rust/telemetry/src/property.rs` - Property type implementation

2. **Processing & Analytics**:
   - `rust/analytics/src/lakehouse/partition_source_data.rs` - Partitioning logic
   - `rust/analytics/src/lakehouse/jit_partitions.rs` - JIT materialization
   - `rust/analytics/src/log_entries_table.rs` - Log entries with properties
   - `rust/analytics/src/metrics_table.rs` - Metrics with properties

3. **UDFs & Queries**:
   - `rust/analytics/src/properties/property_get.rs` - Extract properties
   - `rust/analytics/src/properties/properties_to_dict_udf.rs` - Dictionary conversion
   - `rust/analytics/src/arrow_properties.rs` - Arrow property utilities

## Current Limitations
- Properties stored as full strings (no deduplication)
- Each property key/value pair stored separately
- No compression of repeated values
- Memory overhead for high-cardinality properties

## Migration Requirements
To convert to dictionary encoding:
1. Update Arrow schema to use `Dictionary<Int32, Utf8>` for keys/values
2. Modify table builders to use dictionary builders
3. Update SQL Arrow bridge for dictionary output
4. Ensure UDFs handle dictionary-encoded arrays
5. Maintain backward compatibility for existing data