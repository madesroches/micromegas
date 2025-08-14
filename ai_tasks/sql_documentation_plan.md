# Micromegas SQL Documentation Plan

## Overview
Plan for creating comprehensive documentation of Micromegas's SQL capabilities, including the Python API interface, available views, schema design, and query patterns. Users of the Grafana plugin can also leverage the same SQL capabilities.

## Current Status
🔄 **PLANNING PHASE** - Documentation needs to be created from scratch

## Objectives

### 1. Core SQL Interface Documentation
- Clarify that Micromegas SQL is an extension of DataFusion's SQL
- Link to DataFusion SQL documentation
- Document the Python API for SQL query execution
- Document return types (pandas DataFrame)
- Document query streaming capabilities and when to use them
- SQL query execution and response format
- Supported SQL features and limitations
- Mention Grafana plugin SQL capabilities

### 2. Schema Documentation
- Core tables: `processes`, `streams`, `thread_events`
- Available views: `async_events`, `spans`, `metrics`, `logs`
- Relationship mappings between tables

### 3. Data Structures and Functions
- Custom data types used in Micromegas
- Available SQL functions specific to observability data
- Time-based functions and operations
- Aggregation functions for telemetry data
- Extensions beyond standard DataFusion functions

### 4. Query Patterns and Examples
- Common observability queries (filtering, aggregation, time-based)
- Performance optimization guidelines
- JOIN patterns between views
- Best practices for high-frequency data queries

### 5. View System Documentation
- View materialization strategies
- Global views materialized by the maintenance service
- JIT (Just-In-Time) view instances for specific queries
- Process-scoped views and partitioning
- View factory system
- Custom view creation

## Tasks

### Phase 1: Core Documentation Structure
- [x] Choose framework & style for documentation (Hierarchical Reference Structure)
- [x] Create main SQL documentation file following DataFusion's style
- [x] Establish early that Micromegas SQL extends DataFusion SQL
- [x] Link to DataFusion SQL documentation
- [x] Document Python API for SQL queries
- [x] Document return types (pandas DataFrame)
- [ ] Document query streaming capabilities and usage patterns
- [x] Create hierarchical table of contents with deep links
- [x] Document basic schema overview
- [x] Add query examples using Python API
- [x] Mention Grafana plugin SQL usage

### Phase 2: Schema Deep Dive
- [ ] Document all core views with field descriptions (hierarchical format)
- [ ] Create alphabetical function lists at section tops
- [ ] Document all available views with standardized format
- [ ] Explain dictionary compression and optimization strategies
- [ ] Document data flow from ingestion to query
- [ ] Add cross-references between related views

### Phase 3: Data Structures and Functions
- [ ] Document custom data types using DataFusion-style format
- [ ] List observability-specific SQL functions alphabetically
- [ ] Use consistent function signature format in code blocks
- [ ] Document time-based functions with executable examples
- [ ] Document aggregation functions with expected output
- [ ] Explain extensions beyond standard DataFusion functions
- [ ] Add cross-references between related functions

### Phase 4: Query Examples and Patterns
- [ ] Create searchable query cookbook with consistent format
- [ ] Document JOIN strategies with executable examples
- [ ] Add performance optimization guidelines with explanations
- [ ] Include troubleshooting section with common issues
- [ ] Use DataFusion-style argument descriptions format
- [ ] Add brief, clear descriptions for each pattern

### Phase 5: Performance Pitfalls and Optimization
- [ ] Document performance pitfalls with specific examples
- [ ] Queries without time ranges - memory usage and query time impacts
- [ ] ORDER BY performance implications on large datasets
- [ ] JOIN performance considerations and optimization strategies
- [ ] GROUP BY performance patterns and memory allocation issues
- [ ] Resource consumption patterns and system stability concerns
- [ ] Best practices for avoiding performance bottlenecks
- [ ] Query planning and execution optimization tips

### Phase 6: Advanced Features
- [ ] Document view materialization strategies
- [ ] Explain global views and maintenance service
- [ ] Document JIT view instances and when they're used
- [ ] Document custom view creation

## Target Audience
- Data analysts querying telemetry data
- DevOps teams setting up observability
- Contributors to the Micromegas project

## Documentation Location
- Single location: `doc/how_to_query/` (hierarchical reference structure)

## Style Guidelines (DataFusion-inspired)
- **Function signatures** in code blocks
- **Consistent argument descriptions** with type information
- **Executable examples** with expected output
- **Cross-references** between related functions and views
- **Alphabetical organization** within categories
- **Brief, clear descriptions** for immediate understanding
- **Deep linking** for easy navigation
- **Searchable structure** with clear hierarchies

