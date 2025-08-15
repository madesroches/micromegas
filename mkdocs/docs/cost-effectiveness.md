# Cost Effectiveness

Micromegas is designed to provide enterprise-grade observability at a fraction of the cost of commercial SaaS platforms by leveraging direct infrastructure costs rather than abstracted pricing models.

## Cost Philosophy

Unlike traditional observability platforms that charge per GB ingested, per host, or per user, **Micromegas runs on your own infrastructure**. Your cost is simply the direct cost of the cloud services you consume.

### Why This Matters

- **Full transparency** - See every dollar spent on your cloud bill
- **No vendor margins** - Pay only for actual infrastructure usage
- **Predictable scaling** - Costs scale linearly with resource consumption
- **Data ownership** - Your telemetry data never leaves your cloud account

## Primary Cost Drivers

The infrastructure cost for Micromegas comes from standard cloud services:

### Compute Services

- **Ingestion Service** (`telemetry-ingestion-srv`) - Handles incoming telemetry data
- **Analytics Service** (`flight-sql-srv`) - Serves SQL queries and dashboards
- **Maintenance Daemon** (`telemetry-admin`) - Background data processing and rollups

### Storage Services

- **Database (PostgreSQL)** - Stores metadata about processes, streams, and data blocks
- **Object Storage (S3/GCS)** - Stores raw telemetry payloads and materialized Parquet files

### Supporting Infrastructure

- **Load Balancers** - Route traffic to services
- **Networking** - Data transfer and connectivity

## Example Deployment Cost

Here's a real-world cost breakdown for a production Micromegas deployment:

### Data Scale

- **Retention Period:** 90 days
- **Total Storage:** 8.5 TB in 118 million objects
- **Log Entries:** 9 billion
- **Metric Events:** 275 billion  
- **Trace Events:** 165 billion

### Monthly Infrastructure Costs

| Component | Specification | Monthly Cost |
|-----------|---------------|--------------|
| **Ingestion Services** | 2 instances × (1 vCPU, 2GB RAM) | ~$30 |
| **Analytics Service** | 1 instance × (4 vCPU, 8GB RAM) | ~$120 |
| **Maintenance Daemon** | 1 instance × (4 vCPU, 8GB RAM) | ~$120 |
| **PostgreSQL Database** | Aurora Serverless (44GB storage) | ~$200 |
| **Object Storage** | 8.5TB S3 Standard + requests | ~$500 |
| **Load Balancer** | Application Load Balancer | ~$30 |
| **Total** | | **~$1,000/month** |

### Scale Perspective

This deployment handles:

- **449 billion total events** over 90 days
- **~165 million events per day**
- **~1,900 events per second** average throughput

## Cost Management Features

### On-Demand Processing (Tail Sampling)

Micromegas supports storing all raw telemetry data in low-cost object storage and materializing it for analysis only when needed:

- **Raw data** stored cheaply in S3/GCS
- **Processing costs** only when querying specific data
- **Selective materialization** based on actual analysis needs

### Flexible Retention Policies

Configure retention periods independently for:

- **Raw telemetry data** - Keep longer in cheap storage
- **Materialized views** - Shorter retention for frequently accessed data
- **Metadata** - Configure based on compliance requirements

## Commercial Platform Comparison

### Pricing Model Differences

| Aspect | Commercial SaaS | Micromegas |
|--------|-----------------|------------|
| **Cost Basis** | Per-GB, per-host, per-user | Direct infrastructure costs |
| **Transparency** | Opaque vendor margins | Full cost visibility |
| **Control** | Limited infrastructure control | Complete infrastructure control |
| **Scalability** | Vendor-managed, unpredictable costs | Self-managed, predictable scaling |
| **Data Ownership** | Third-party hosted | Your cloud account only |

### When Micromegas is Cost Effective

The Micromegas model is particularly advantageous when:

- **High data volumes** - Direct infrastructure costs scale better than per-GB pricing
- **Cost predictability** is critical for budgeting
- **Data governance** requirements favor keeping data in your environment
- **Operational maturity** exists to manage distributed systems
- **Long-term retention** is needed (cheap object storage vs. expensive SaaS retention)

## Detailed Cost Comparisons

For in-depth, dollar-for-dollar comparisons with specific platforms:

- [Micromegas vs. Datadog](cost-comparisons/datadog.md)
- [Micromegas vs. Dynatrace](cost-comparisons/dynatrace.md)
- [Micromegas vs. Elastic Observability](cost-comparisons/elastic.md)
- [Micromegas vs. Grafana Cloud](cost-comparisons/grafana.md)
- [Micromegas vs. New Relic](cost-comparisons/newrelic.md)
- [Micromegas vs. Splunk](cost-comparisons/splunk.md)

## Getting Started with Cost Optimization

1. **Start small** - Deploy minimal infrastructure and scale as needed
2. **Monitor usage** - Use cloud billing dashboards to track costs
3. **Optimize retention** - Balance storage costs with analysis needs  
4. **Leverage tail sampling** - Store everything, process selectively
5. **Right-size compute** - Match instance types to actual workload demands

The goal is predictable, transparent costs that scale efficiently with your observability needs.