# Python API Documentation Plan

## Current Status
- Documentation site: https://madesroches.github.io/micromegas/docs/query-guide/python-api/
- **Phase 1 Complete**: All Python docstrings added (100% coverage of public API methods)
- **Remaining**: Update documentation website with new methods and examples

## Phase 1: Add Python Docstrings ✅ COMPLETED

### 1.1 FlightSQLClient Methods (`python/micromegas/micromegas/flightsql/client.py`)

#### Core Methods
- [x] `__init__(uri, headers=None)` - Constructor with connection configuration
- [x] `query(sql, begin=None, end=None)` - Execute SQL and return pandas DataFrame
- [x] `query_stream(sql, begin=None, end=None)` - Stream results as Arrow RecordBatch

#### Prepared Statements
- [x] `prepare_statement(sql)` - Create prepared statement for repeated execution
- [x] `prepared_statement_stream(statement)` - Stream prepared statement results

#### Data Management
- [x] `bulk_ingest(table_name, df)` - Bulk ingest pandas DataFrame into table
- [x] `retire_partitions(view_set_name, view_instance_id, begin, end)` - Remove partitions
- [x] `materialize_partitions(view_set_name, begin, end, partition_delta_seconds)` - Create materialized views

#### Specialized Queries
- [x] `find_process(process_id)` - Find process by ID
- [x] `query_streams(begin, end, limit, process_id=None, tag_filter=None)` - Query event streams
- [x] `query_blocks(begin, end, limit, stream_id)` - Query blocks within stream
- [x] `query_spans(begin, end, limit, stream_id)` - Query thread spans

### 1.2 PreparedStatement Class (`python/micromegas/micromegas/flightsql/client.py`)
- [x] Class docstring
- [x] `query` property - SQL query string (documented via class docstring)
- [x] `dataset_schema` property - PyArrow schema (documented via class docstring)

### 1.3 Time Utilities (`python/micromegas/micromegas/time.py`)
- [x] `format_datetime(value)` - Format datetime for queries
- [x] `parse_time_delta(user_string)` - Parse time strings like "1h", "30m"

### 1.4 Perfetto Integration (`python/micromegas/micromegas/perfetto.py`)
- [x] Existing docstrings already comprehensive (includes span_types parameter)

## Phase 2: Update Documentation Website ✅ COMPLETED

### 2.1 Update Existing Page (`mkdocs/docs/query-guide/python-api.md`)

Added sections for all missing API methods:
- [x] **Connection Configuration** - FlightSQLClient constructor with authentication examples
- [x] **Schema Discovery** - prepare_statement() and prepared_statement_stream() workflows
- [x] **Process and Stream Discovery** - find_process(), query_streams(), query_blocks(), query_spans()
- [x] **Data Management** - bulk_ingest(), materialize_partitions(), retire_partitions()  
- [x] **Time Utilities** - format_datetime() and parse_time_delta() with examples

### 2.2 Create Advanced Features Page (`mkdocs/docs/query-guide/python-api-advanced.md`)

Created comprehensive advanced guide covering:
- [x] **Advanced Connection Patterns** - Authentication, headers, connection pooling
- [x] **Schema Discovery and Query Validation** - Advanced introspection and validation pipelines
- [x] **Performance Optimization** - Intelligent batching, memory-efficient processing
- [x] **Advanced Data Management** - Automated partition management, bulk migration tools
- [x] **Perfetto Integration** - Advanced trace generation and performance analysis
- [x] **Error Handling and Resilience** - Retry logic, fallback strategies

### 2.3 Update Navigation (`mkdocs/mkdocs.yml`)
- [x] Added "Python API Advanced" under Query Guide section
- [x] Added cross-references between basic and advanced pages

## Phase 3: Examples and Best Practices

### 3.1 Code Examples
For each method, provide:
- Basic usage example
- Advanced usage with error handling
- Performance considerations
- Common pitfalls

### 3.2 Integration Examples
- Jupyter notebook workflows
- Data pipeline integration
- Monitoring dashboards
- Alerting systems

## Phase 4: Quality Assurance

### 4.1 Code Quality
- [ ] Run black formatter on all modified Python files
- [ ] Ensure all docstrings follow Google/NumPy style
- [ ] Verify type hints are accurate

### 4.2 Documentation Quality
- [ ] Test all code examples
- [ ] Verify links between pages
- [ ] Check for consistent terminology
- [ ] Ensure examples use actual API

## Files to Modify

### Python Source Files
1. `python/micromegas/micromegas/flightsql/client.py` - Add comprehensive docstrings
2. `python/micromegas/micromegas/time.py` - Document utility functions
3. `python/micromegas/micromegas/perfetto.py` - Enhance existing docs

### Documentation Files
1. `mkdocs/docs/query-guide/python-api.md` - Expand with all methods
2. `mkdocs/docs/query-guide/python-api-advanced.md` - Create new advanced page
3. `mkdocs/mkdocs.yml` - Update navigation structure

## Success Metrics
- 100% of public API methods documented with docstrings
- All methods included in website documentation
- Working examples for every method
- Clear migration path from basic to advanced usage

## Implementation Order
1. Add docstrings to Python source (enables IDE help)
2. Update basic documentation page
3. Create advanced features page
4. Add comprehensive examples
5. Run formatters and validate

## Estimated Effort
- Phase 1 (Docstrings): 2-3 hours
- Phase 2 (Documentation): 3-4 hours  
- Phase 3 (Examples): 2-3 hours
- Phase 4 (QA): 1 hour

Total: ~10-11 hours of focused work