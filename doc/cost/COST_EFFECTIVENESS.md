## Cost-Effectiveness

The infrastructure cost for Micromegas will vary depending on your specific usage patterns, data volume, and retention policies. The system is designed to be scalable, allowing you to start small and grow as your needs evolve. For a qualitative comparison of this cost model with other observability solutions, see the [Cost Modeling](./COST_MODELING.md) document.

The primary cost drivers are the cloud services required to run the backend:

*   **Ingestion Service (`telemetry-ingestion-srv`):** Handles incoming data from your instrumented applications.
    *   **Cost Factors:** Number of connected clients, volume of telemetry data (logs, traces, metrics).
    *   **Notes:** This service can autoscale based on load. It may have occasional CPU peaks when processing large trace payloads.

*   **Analytics Service (`flight-sql-srv`):** Serves queries from users, dashboards (e.g., Grafana), and the command-line interface.
    *   **Cost Factors:** Number of concurrent users, complexity and frequency of queries.
    *   **Notes:** This service is often idle but will experience CPU peaks during heavy querying.

*   **Daemon (`telemetry-admin-cli`):** Runs background tasks like data rollup, expiration, and view materialization.
    *   **Cost Factors:** Amount of data being processed for rollups and maintenance.
    *   **Notes:** CPU usage is typically low but will peak periodically (e.g., hourly) when maintenance tasks run.

*   **Database (PostgreSQL):** Stores metadata about processes, streams, and data blocks.
    *   **Cost Factors:** Size of the event batches, number of instrumented processes, and the total amount of data stored.

*   **Blob Storage (S3, GCS, etc.):** Stores the raw telemetry payloads and materialized Parquet files for querying.
    *   **Cost Factors:** Total data volume, data retention period, number of objects (files).

### Example Deployment

Here is a snapshot of the estimated monthly costs associated with a typical Micromegas deployment. In this scenario, all logs and metrics were processed and materialized every second, while the traces were left in object storage to be processed on-demand.

*   **Data Volume & Retention:**
    *   **Retention Period:** 90 days
    *   **Total S3 Storage:** 8.5 TB in 118 million objects. This includes raw payloads and materialized Parquet views.
    *   **Log Entries:** 9 billion
    *   **Metric Events (measures):** 275 billion
    *   **Trace Events:** 165 billion

*   **Compute Resources (Virtual Machines):**
    *   **Ingestion:** 2 instances, each with 1 vCPU and 2 GB of RAM.
        *   *Estimated Cost: ~$30 / month*
    *   **Analytics (flight-sql-srv):** 1 instance with 4 vCPU and 8 GB of RAM.
        *   *Estimated Cost: ~$120 / month*
    *   **Daemon (telemetry-admin):** 1 instance with 4 vCPU and 8 GB of RAM.
        *   *Estimated Cost: ~$120 / month*

*   **Database (Aurora PostgreSQL Serverless):**
    *   **Storage Volume:** 44 GB
        *   *Estimated Cost: ~$200 / month (including compute and I/O)*

*   **Blob Storage (S3 Standard):**
    *   **Storage:** 8.5 TB
    *   **Requests:** ~40 million new objects per month
        *   *Estimated Cost: ~$500 / month (including storage and requests)*

*   **Load Balancer:**
    *   *Estimated Cost: ~$30 / month*

**Total Estimated Monthly Cost: ~$1000 / month**

### Cost Management

Micromegas includes features designed to help control costs:

*   **On-Demand Processing (tail sampling):** Raw telemetry data, especially detailed traces, can be stored unprocessed in low-cost object storage. This data is materialized for analysis on-demand using a SQL extension, which significantly reduces processing costs, as you only pay for what you query.
*   **Data Retention:** You can configure the retention period for both raw data and materialized views to balance cost and analysis needs.