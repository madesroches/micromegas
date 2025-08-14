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
- [ ] Choose framework & style for documentation
- [ ] Create main SQL documentation file
- [ ] Establish early that Micromegas SQL extends DataFusion SQL
- [ ] Link to DataFusion SQL documentation
- [ ] Document Python API for SQL queries
- [ ] Document basic schema overview
- [ ] Add query examples using Python API
- [ ] Mention Grafana plugin SQL usage

### Phase 2: Schema Deep Dive
- [ ] Document all core views with field descriptions
- [ ] Document all available views
- [ ] Explain dictionary compression and optimization strategies
- [ ] Document data flow from ingestion to query

### Phase 3: Data Structures and Functions
- [ ] Document custom data types used in Micromegas
- [ ] List and explain observability-specific SQL functions
- [ ] Document time-based functions and operations
- [ ] Document aggregation functions for telemetry data
- [ ] Explain extensions beyond standard DataFusion functions

### Phase 4: Query Examples and Patterns
- [ ] Create query cookbook with common patterns
- [ ] Document JOIN strategies between views
- [ ] Add performance optimization guidelines
- [ ] Include troubleshooting section

### Phase 5: Advanced Features
- [ ] Document view materialization strategies
- [ ] Explain global views and maintenance service
- [ ] Document JIT view instances and when they're used
- [ ] Document custom view creation

## Target Audience
- Data analysts querying telemetry data
- DevOps teams setting up observability
- Contributors to the Micromegas project

## Documentation Location
- Primary location: `doc/how_to_query/`

