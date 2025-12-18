# Changelog

This file documents the historical progress of the Micromegas project. For current focus, please see the main [README.md](./README.md).

## [Unreleased]
* **Tracing & Instrumentation:**
  * Improve #[span_fn] rustdoc documentation (#676)
  * Fix async span parenting and add spawn_with_context helper (#675)
* **Analytics & Query Features:**
  * Add global LRU metadata cache for partition metadata (#674)
  * Add jsonb_object_keys UDF (#673)
* **Analytics Web App:**
  * Migrate from Next.js to Vite for dynamic base path support (#667)
  * Improve process info navigation and cleanup trace screen (#669)
  * Fix custom queries being reset when filters change (#670)
* **Unreal Engine:**
  * Add more metrics and process info to telemetry plugin (#672)
* **Security:**
  * Fix esbuild security vulnerability (GHSA-67mh-4wv8-2f99) (#671)
* **Planning:**
  * Add user-defined screens feature plan (#665)

## December 2025 - v0.17.0
 * **Analytics Web App Major Rework:**
   * Complete UI redesign with dark theme and Micromegas branding (#621, #622, #623)
   * Add Grafana-style time range picker with relative and absolute time support (#631)
   * Add performance analysis screen with thread coverage timeline (#642, #643)
   * Add Perfetto trace integration with split button for browser/download (#660, #661)
   * Add process metrics screen with time-series charting (#639)
   * Add process properties display panel (#634)
   * Add multi-word search to process list and log screens (#632, #633)
   * Allow custom limit values in process log view (#627, #628)
   * Improve time column formatting in process logs (#624)
   * Pass time range through process navigation links (#636)
   * Add schema documentation links to SQL panels (#635)
   * UX improvements and polish (#645, #647)
 * **Deployment & Configuration:**
   * Add per-service Docker images and modernize build scripts (#637, #649)
   * Add BASE_PATH and MICROMEGAS_PORT env vars for reverse proxy deployments (#650, #651, #654, #656, #658, #659)
 * **Unreal Engine:**
   * Add scalability and VSync context to telemetry (#625)
   * Document API key authentication (#629)
 * **Security & Bug Fixes:**
   * Fix CVE-2025-66478: Update Next.js to 15.5.7 (#626)
   * Fix urllib3 security vulnerabilities and OIDC token validation bug (#641)
   * Fix UTF-8 user attribution headers with percent-encoding (#638)
   * Handle empty MICROMEGAS_TELEMETRY_URL environment variable (#644)
 * **Documentation:**
   * Fix documentation dark mode readability (#648)
 * **Code Quality:**
   * Fix rustdoc bare URL warnings in auth crate (#630)

## November 2025 - v0.16.0
 * Released [version 0.16.0](https://crates.io/crates/micromegas)
 * **New: HTTP Gateway:**
   * Add HTTP Gateway with Authentication and Security Features (#597)
 * **Analytics Web App:**
   * Add OIDC authentication to analytics web app (#596)
 * **Authentication:**
   * Fix ID token expiration and add multi-provider OIDC support (#608)
   * Fix OIDC authentication and token refresh issues (#590)
 * **Analytics & Query Features:**
   * Optimize JSONB UDFs for dictionary-encoded column support (#593)
   * Fix timestamp binding in retire_partition_by_metadata UDF (#606)
   * Handle empty incompatible partitions and fix thrift buffer sizing (#602)
 * **Grafana Plugin:**
   * Fix Grafana plugin packaging and document release process (#601)
   * Fix secureJsonData undefined error and rename plugin to Micromegas FlightSQL (#603)
 * **Security & Dependencies:**
   * Fix js-yaml prototype pollution vulnerability (CVE-2025-64718) (#592)
   * Upgrade DataFusion from version 50.2.0 to 51.0.0 (#598)
   * Fix LIMIT pushdown in all TableProvider implementations (#600)
 * **Documentation:**
   * Document auth_provider parameter and deprecate headers in Python API (#595)
 * **Build & CI:**
   * Enable Claude to submit PR reviews and issue comments (#605)
   * Claude PR Assistant workflow (#604)

## November 2025 - v0.15.0
 * Released [version 0.15.0](https://crates.io/crates/micromegas)
 * **New: Authentication Framework (micromegas-auth crate):**
   * Add authentication framework with OIDC and API key support (#546)
   * Implement OIDC authentication for Rust services and Python client (#548)
   * Add OIDC authentication support to CLI tools (#549)
   * Add OAuth 2.0 client credentials support for service accounts (#552)
   * Add HTTP authentication to ingestion service (#551)
   * Unified JWKS architecture for service accounts (#547)
   * Refactor OIDC connection to library module (#588)
 * **Grafana Plugin (v0.15.0 - First release from main repo):**
   * Integrate Grafana FlightSQL datasource plugin into main repository (#554)
   * Implement OAuth 2.0 authentication for Grafana plugin (#564)
   * Add variable query editor and datasource migration tools (#585)
   * Rename Grafana plugin to follow official naming guidelines (#583)
   * Implement CI/CD pipeline for Grafana plugin (#558)
   * Update Grafana plugin SDK to 11.6.7 and fix security vulnerabilities (#555)
   * Fix 28 Dependabot security vulnerabilities (#556)
 * **Authentication & Security:**
   * Rework AuthProvider to use request validation (#571)
   * Refactor MultiAuthProvider for extensibility (#569)
   * Add client IP logging to server observability (#566)
   * Comprehensive authentication documentation in admin guide (#550)
 * **Unreal Engine:**
   * Modernize Unreal telemetry sink module (#584)
 * **Server Enhancements:**
   * Add gRPC health check endpoint (#570)
 * **Build & CI:**
   * Fix CI linker crashes and improve build reliability (#572)
   * Fix documentation build by installing mold linker (#573)
 * **Documentation:**
   * Add build tools installation before build steps (#582)
   * Update build prerequisites (#581)
   * Add documentation links to all Rust crate READMEs (#578)
   * Update high-frequency observability presentation (#574)
   * Clean up presentation files and update docs to use yarn (#568)
   * Clean up task documentation and improve authentication docs (#567)
   * Consolidate and streamline Grafana and monorepo documentation (#559)
   * Update documentation links to use hosted docs and fix markdown formatting (#563)
   * Remove Grafana section from README (#560)
 * **Planning:**
   * Add plan for query variable time filter feature (#580)
   * Grafana plugin repository merge planning and Phase 1.1 completion (#553)

## October 2025 - v0.14.0
 * Released [version 0.14.0](https://crates.io/crates/micromegas)
 * **Performance & Storage Optimizations:**
   * Complete properties to dictionary-encoded JSONB migration (#521)
   * Properties writing optimization with ProcessMetadata and BinaryColumnAccessor (#522, #524)
 * **Analytics & Query Features:**
   * Add Dictionary<Int32, Binary> support to jsonb_format_json UDF (#536)
   * Add SessionConfigurator for custom table registration (#531)
   * Add file existence validation to json_table_provider (#532)
   * Enable property_get UDF to access JSONB columns (#520)
   * Add support for empty lakehouse partitions (#537)
 * **Bug Fixes & Reliability:**
   * Fix NULL value handling in SQL-Arrow bridge with integration tests (#541)
   * Fix null decoding error in list_partitions table function (#540)
   * Fix null decoding error for file_path in retire_partitions (#539)
 * **Documentation & Presentations:**
   * Add High-Frequency Observability presentation (OSACON 2025) (#527, #528, #529, #533)
   * Update presentation template to new Vite-based build (#525)
 * **Security & Dependencies:**
   * Update Vite to 7.1.11 to fix security vulnerabilities (#526, #542)
   * Update DataFusion and Arrow Flight dependencies (#519)
   * cargo update (#530)
 * **Code Quality:**
   * Fix rustdoc HTML tag warnings in analytics crate (#534)
 * **Future Work:**
   * Analytics Server Authentication Plan (#543)

## September 2025 - v0.13.0
 * Released [version 0.13.0](https://crates.io/crates/micromegas)
 * **Performance & Storage Optimizations:**
   * Dictionary encoding for properties columns with comprehensive UDF support (#506, #507, #508, #510, #511)
   * Properties to JSONB UDF for efficient storage and querying (#515)
   * Arrow string column accessor with full dictionary encoding support (#511)
   * Production performance analysis of dictionary encoding effectiveness (#508)
   * Fixed parquet metadata race conditions with separation strategy (#502, #504)
   * Optimized lakehouse partition queries by removing unnecessary file_metadata fetches (#499)
   * Scalability improvements for high-volume environments (#497, #498)
 * **Schema Evolution & Admin Features:**
   * Incompatible partition retirement feature for schema evolution (#512)
   * Enhanced error logging in CompletionTrackedStream (#503)
   * Improved PostgreSQL container management in development environment (#500)
 * **Monitoring & Analytics:**
   * Added `log_stats` SQL aggregation view for log analysis by severity and service (#495, #505)
   * Enhanced documentation with log_stats view in schema reference
 * **Code Quality & Development:**
   * Organized project documentation and completed task archival (#513, #514, #517)
   * Dictionary encoding analysis archived due to Parquet limitations (#516)

## September 2025
 * Released [version 0.12.0](https://crates.io/crates/micromegas)
 * **Major Features:**
   * Comprehensive async span tracing with `micromegas_main` proc macro (#451)
   * Named async span event tracking with improved API ergonomics (#475)
   * Async span depth tracking for performance analysis (#474)
   * Async trait tracing support in `span_fn` macro (#469)
   * Perfetto async spans support with trace generation (#485)
   * HTTP gateway for easier interoperability (#433, #435, #436)
   * JSONB support for flexible data structures (#409)
 * **Infrastructure & Performance:**
   * Consolidate Perfetto trace generation to use SQL-powered implementation (#489)
   * Query latency tracking and async span instrumentation optimization (#468)
   * Replace custom interning logic with `internment` crate (#430)
   * Optimize view_instance metadata requests (#450)
   * Convert all unit tests to in-memory recording (#472)
 * **Documentation & Developer Experience:**
   * Complete Python API documentation with comprehensive docstrings (#491)
   * Complete SQL functions documentation with all missing UDFs/UDAFs/UDTFs (#470)
   * Visual architecture diagrams in documentation (#462)
   * Unreal instrumentation documentation (#492)
   * Automated documentation publishing workflow (#444)
 * **Security & Dependencies:**
   * Fix CVE-2025-58160: Update tracing-subscriber to 0.3.20 (#490)
   * Update DataFusion, tokio and other dependencies (#429, #476)
   * Rust edition 2024 upgrade with unsafe operations fixes (#408)
 * **Web UI & Export:**
   * Export Perfetto traces from web UI (#482)
   * Analytics web app build fixes and documentation updates (#483)
 * **Cloud & Deployment:**
   * Docker deployment scripts (#422)
   * Amazon Linux setup script (#423)
   * Cloud environment configuration support (#426)
   * Configurable PostgreSQL port via MICROMEGAS_DB_PORT (#425)

## July 2025
 * Released [version 0.11.0](https://crates.io/crates/micromegas)
 * Working on http gateway for easier interoperability
 * Add export mechanism to view materialization to send data out as it is ingested

## June 2025
 * Released [version 0.10.0](https://crates.io/crates/micromegas)
 * Process properties in measures and log_entries
 * Better histogram support
 * Processes and streams views now contain all processes/streams updated in the requested time range - based on SqlBatchView.

## May 2025
 * Released [version 0.8.0](https://crates.io/crates/micromegas) and [version 0.9.0](https://crates.io/crates/micromegas)
 * Frame budget reporting
 * Histogram support with quantile estimation
 * Run seconds & minutes tasks in parallel in daemon
 * GetPayload user defined function
 * Add bulk ingestion API for replication

## April 2025
 * Released [version 0.7.0](https://crates.io/crates/micromegas)
 * Perfetto trace server
 * DataFusion memory budget
 * Memory optimizations
 * Fixed interning of property sets
 * More flexible trace macros

## March 2025
 * Released [version 0.5.0](https://crates.io/crates/micromegas)
 * Better perfetto support
 * New rust FlightSQL client
 * Unreal crash reporting

## February 2025
 * Released [version 0.4.0](https://crates.io/crates/micromegas)
 * Incremental data reduction using sql-defined views
 * System monitor thread
 * Added support for ARM (& macos)
 * Deleted analytics-srv and the custom http python client to connect to it
 
## January 2025
 * Released [version 0.3.0](https://crates.io/crates/micromegas)
 * New FlightSQL python API
   * Ready to replace analytics-srv with flight-sql-srv

## December 2024
 * [Grafana plugin](https://github.com/madesroches/micromegas/tree/main/grafana)
 * Released [version 0.2.3](https://crates.io/crates/micromegas)
 * Properties on measures & log entries available in SQL queries

## November 2024
Released [version 0.2.1](https://crates.io/crates/micromegas)

 * FlightSQL support
 * Measures and log entries can now be tagged with properties
   * Not yet available in SQL queries

## October 2024
Released [version 0.2.0](https://crates.io/crates/micromegas)

 * Unified the query interface
   * Using `view_instance` table function to materialize just-in-time process-specific views from within SQL
 * Updated python doc to reflect the new API: https://pypi.org/project/micromegas/

## September 2024
Released [version 0.1.9](https://crates.io/crates/micromegas)

 * Updating global views every second
 * Caching metadata (processes, streams & blocks) in the lakehouse & allow sql queries on them

## August 2024
Released [version 0.1.7](https://crates.io/crates/micromegas)

 * New global materialized views for logs & metrics of all processes
 * New daemon service to keep the views updated as data is ingested
 * New analytics API based on SQL powered by Apache DataFusion

## July 2024
Released [version 0.1.5](https://crates.io/crates/micromegas)

Unreal
 * Better reliability, retrying failed http requests
 * Spike detection

Maintenance
 * Delete old blocks, streams & processes using cron task

## June 2024
Released [version 0.1.4](https://crates.io/crates/micromegas)

Good enough for dogfooding :)

Unreal
 * Metrics publisher
 * FName scopes

Analytics
 * Metric queries
 * Convert cpu traces in perfetto format

## May 2024
Released [version 0.1.3](https://crates.io/crates/micromegas)

Better unreal engine instrumentation
  * new protocol
  * http request callbacks no longer binded to the main thread
  * custom authentication of requests

Analytics
  * query process metadata
  * query spans of a thread

## April 2024
Telemetry ingestion from rust & unreal are working :) 

Released [version 0.1.1](https://crates.io/crates/micromegas)

Not actually useful yet, I need to bring back the analytics service to a working state.

## January 2024
Starting anew. I'm extracting the tracing/telemetry/analytics code from https://github.com/legion-labs/legion to jumpstart the new project. If you are interested in collaborating, please reach out.
