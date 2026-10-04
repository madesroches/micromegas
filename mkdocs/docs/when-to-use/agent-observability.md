# Micromegas for LLM Agent Observability

Micromegas records what an LLM agent emits over OTLP/HTTP (Claude Code is the worked example) into the same SQL-queryable lakehouse as the rest of your telemetry, with per-row access control. It is not an LLM engineering platform: it has no evaluations, prompt management or LLM-specific trace UI. This page compares it with Langfuse, Arize Phoenix, Opik and Laminar, which do.

**TL;DR.** Choose Langfuse, Phoenix, Opik or Laminar when you want evals, datasets, prompt management and a purpose-built trace UI. Choose Micromegas when you want agent telemetry stored with the rest of your fleet's telemetry, queried with the same SQL, and readable only by the people it is shared with. Many teams will want both; see [Using them together](#using-them-together). The [summary at the end](#summary-which-one-fits) lists when each choice fits.

*Last reviewed: October 2026*

Every claim on this page links to its source: the project's own docs or repository for the peers, and the Micromegas docs or source code for Micromegas. If something is wrong or out of date, please [open an issue](https://github.com/madesroches/micromegas/issues).

## What Micromegas records from an agent

- **Agent telemetry over OTLP/HTTP.** An agent that exports OpenTelemetry can send to Micromegas. Claude Code is the worked example: its API requests, with model, token counts and cost, land as `log_entries` and `measures` rows, and its beta traces land as `otel_spans` ([OTLP recipe](../otlp/index.md#claude-code)). Claude Code exports `claude_code.cost.usage` and `claude_code.token.usage` metrics and `user_prompt`, `assistant_response`, `api_request` and `tool_result` events, and traces are in beta ([Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage)). Prompt, response and tool content is off by default; it is recorded only when the matching flag is set (`OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES`, `OTEL_LOG_TOOL_DETAILS`, and `OTEL_LOG_TOOL_CONTENT` for tool output), as the [blog post on recording an agent](../blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md) does.
- **Per-row audiences.** Every row is stamped server-side from the ingestion credential, so a producer cannot forge it ([audience stamping](../admin/authorization.md#audience-stamping)). Rows are private by default, sharing is an edit to read grants rather than a rewrite of data, and metrics can go to a team audience while prompt-bearing logs go to a personal one ([authorization](../admin/authorization.md), [blog post](../blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md)).
- **One SQL surface for everything.** Agent data sits next to the game servers and CI runners the agent touches, queried with the same SQL and [notebooks](../web-app/index.md). Analysis of the agent's own sessions is done by pointing an agent at the data through the CLI and a skill file ([from o11y to candor](../blog/posts/2026-03-29-from-o11y-to-candor.md)).

**Limits**, stated plainly (the general ones are in [Micromegas in brief](index.md#micromegas-in-brief)):

- No evaluations (LLM-as-judge, datasets, experiments), no prompt management or versioning, no playground and no annotation queues.
- No LLM-specific trace UI: no chat-transcript view and no tool-call tree. Agent data is shown with SQL, notebooks and Grafana.
- No GenAI semantic-convention mapping: `gen_ai.*` attributes land in the generic JSONB `properties` column like any other attribute and are read with the `jsonb_*` functions ([attribute encoding](../otlp/index.md#attribute-encoding)).
- No token-cost price table: cost is whatever the agent reports, and Claude Code reports `claude_code.cost.usage`. An agent that only reports tokens gets no dollar figure.
- OTLP histograms are not materialized, and `otel_spans` is per-process, so a multi-agent trace spanning processes needs a UNION ([OTLP limitations](../otlp/index.md#limitations)).
- PostgreSQL is required. The [monolith](../admin/monolith.md) runs everything in one process next to PostgreSQL, with a local directory (`file:///…`) as the object store. Phoenix can also drop the database server and run on SQLite (see below).

## At a glance

| | License | Self-hosted dependencies | OTLP ingestion | SQL over agent data | Evals | Prompt management |
|---|---|---|---|---|---|---|
| Micromegas | [Apache-2.0](https://github.com/madesroches/micromegas/blob/main/LICENSE) | PostgreSQL; object store can be a local directory ([monolith](../admin/monolith.md)) | HTTP only ([limitations](../otlp/index.md#limitations)) | Yes, FlightSQL and notebooks ([query guide](../query-guide/index.md)) | No | No |
| [Langfuse](#micromegas-vs-langfuse) | MIT except `ee/` ([repo](https://github.com/langfuse/langfuse)) | Postgres, ClickHouse, Redis/Valkey, S3 ([self-hosting](https://langfuse.com/self-hosting)) | HTTP only ([OTel](https://langfuse.com/integrations/native/opentelemetry)) | Not listed ([data access](https://langfuse.com/docs/api-and-data-platform/overview)) | Yes ([repo](https://github.com/langfuse/langfuse)) | Yes ([repo](https://github.com/langfuse/langfuse)) |
| [Arize Phoenix](#micromegas-vs-arize-phoenix) | Elastic License 2.0 ([LICENSE](https://github.com/Arize-ai/phoenix/blob/main/LICENSE)) | SQLite or PostgreSQL ([configuration](https://arize.com/docs/phoenix/self-hosting/configuration)) | HTTP and gRPC ([configuration](https://arize.com/docs/phoenix/self-hosting/configuration)) | Not found in the docs read | Yes ([repo](https://github.com/Arize-ai/phoenix)) | Yes ([repo](https://github.com/Arize-ai/phoenix)) |
| [Opik](#micromegas-vs-opik) | Apache-2.0 ([repo](https://github.com/comet-ml/opik)) | MySQL, Redis, ClickHouse, ZooKeeper, MinIO; Helm for production ([local deployment](https://www.comet.com/docs/opik/self-host/local_deployment)) | HTTP only ([OTel](https://www.comet.com/docs/opik/tracing/opentelemetry/overview)) | No: OQL filters, REST, export ([export](https://www.comet.com/docs/opik/tracing/export_data)) | Yes ([repo](https://github.com/comet-ml/opik)) | Yes ([repo](https://github.com/comet-ml/opik)) |
| [Laminar](#micromegas-vs-laminar) | Apache-2.0 ([repo](https://github.com/lmnr-ai/lmnr)) | Postgres, ClickHouse, Quickwit; Helm adds RabbitMQ, Redis ([hosting](https://laminar.sh/docs/hosting-options)) | OTLP-native, gRPC exporter named ([repo](https://github.com/lmnr-ai/lmnr)) | Yes, ClickHouse-dialect SQL editor ([SQL editor](https://laminar.sh/docs/platform/sql-editor)) | Yes ([repo](https://github.com/lmnr-ai/lmnr)) | Not listed |

Per-row access control, set by the ingestion credential, is not something the peers' docs describe. Langfuse gates even project-level RBAC behind a license key ([license key](https://langfuse.com/self-hosting/license-key)); this is "not found in their docs", not a claim that they cannot do it.

## Micromegas vs. Langfuse

**Langfuse** is an open-source platform for LLM engineering with tracing, prompt management, evaluations, datasets and a playground ([repo](https://github.com/langfuse/langfuse)). The repository is MIT licensed except the `ee` folders. Some features need a license key when self-hosted: project-level RBAC roles, protected prompt labels, data retention policies, audit logs, server-side data masking, SCIM and others ([license key](https://langfuse.com/self-hosting/license-key)). Self-hosting needs PostgreSQL, ClickHouse, Redis or Valkey, and S3-compatible blob storage ([self-hosting](https://langfuse.com/self-hosting)). It ingests OTLP over HTTP (JSON or protobuf; gRPC is not supported) and maps `langfuse.*`, `gen_ai.*`, OpenInference and MLflow attributes ([OpenTelemetry](https://langfuse.com/integrations/native/opentelemetry)).

**Choose Langfuse when** you want evals, datasets, a playground and prompt management in one product, a trace UI built for LLM calls, and mapping of `gen_ai.*` and `langfuse.*` attributes done for you.

**How Micromegas differs.** Micromegas has none of those LLM-specific features and no GenAI attribute mapping. In exchange, agent data shares one store and one SQL surface with the rest of your telemetry, instead of a separate stack with four dependencies. The data-access paths Langfuse lists are CLI, MCP server, public API, SDKs, metrics and observations APIs and exports, with no direct SQL ([data access](https://langfuse.com/docs/api-and-data-platform/overview)); Micromegas is queried with SQL over FlightSQL ([query guide](../query-guide/index.md)). Self-hosted Langfuse keeps data indefinitely by default, and retention policies are an enterprise feature ([data retention](https://langfuse.com/docs/data-retention)). In Micromegas, row-level audiences are in the Apache-2.0 build with no paid tier ([authorization](../admin/authorization.md)).

## Micromegas vs. Arize Phoenix

**Arize Phoenix** covers tracing on OpenTelemetry, evaluation, versioned datasets, experiments, a playground and prompt management ([repo](https://github.com/Arize-ai/phoenix)). Its license is the Elastic License 2.0, which is source-available rather than OSI open source ([LICENSE](https://github.com/Arize-ai/phoenix/blob/main/LICENSE)). The docs state that "Phoenix is free to self-host with no feature limitations"; Arize AX is a separate enterprise option with support ([self-hosting](https://arize.com/docs/phoenix/self-hosting)). It runs on SQLite by default or PostgreSQL, accepts OTLP over HTTP (port 6006) and gRPC (port 4317), and retains traces indefinitely unless `PHOENIX_DEFAULT_RETENTION_POLICY_DAYS` is set ([configuration](https://arize.com/docs/phoenix/self-hosting/configuration)). Its instrumentation comes from OpenInference, with Python, JavaScript, Java and Go support ([repo](https://github.com/Arize-ai/phoenix)).

**Choose Phoenix when** you want a single service with no database server (SQLite), evals, experiments and a playground, and OTLP over gRPC. On PostgreSQL, its footprint matches the Micromegas [monolith](../admin/monolith.md): one service plus PostgreSQL.

**How Micromegas differs.** Micromegas is Apache-2.0 rather than source-available, and its ingestion is HTTP only. It is built for telemetry from many kinds of processes, not only LLM applications, so agent data lands next to everything else and is queried with the same SQL. The Phoenix docs read for this page do not describe an SQL interface over traces, so nothing is claimed either way. Micromegas adds per-row audiences, which are not described in Phoenix's configuration docs.

## Micromegas vs. Opik

**Opik**, from Comet, is an Apache-2.0 LLM observability platform with tracing, datasets, experiments, LLM-as-a-judge metrics, a prompt playground and guardrails ([repo](https://github.com/comet-ml/opik)). Its Docker Compose deployment runs MySQL, Redis, ClickHouse, ZooKeeper and MinIO, and for production the docs recommend the Kubernetes Helm chart ([local deployment](https://www.comet.com/docs/opik/self-host/local_deployment)). OpenTelemetry integration supports HTTP transport only, and the docs warn against the gRPC exporter ([OpenTelemetry](https://www.comet.com/docs/opik/tracing/opentelemetry/overview)). Its SDKs are Python and TypeScript, with OpenTelemetry for Java, Ruby and .NET ([repo](https://github.com/comet-ml/opik)).

**Choose Opik when** you want agent traces with full trace trees, evaluation with LLM-as-judge metrics, and a prompt playground, and you are prepared to run Kubernetes for production. The README describes it as designed for scale, at 40M+ traces per day ([repo](https://github.com/comet-ml/opik)).

**How Micromegas differs.** Opik is queried through SDK search with its own filter language (OQL), REST and exports, with no SQL mentioned ([export](https://www.comet.com/docs/opik/tracing/export_data)). Micromegas offers SQL and notebooks over data that also includes non-agent telemetry. Both ingest OTLP over HTTP only. Micromegas has no evaluations or prompt tooling.

## Micromegas vs. Laminar

**Laminar** is an Apache-2.0, OpenTelemetry-native agent observability platform written in Rust, with tracing, evals, dashboards, annotation and datasets, and SQL-based querying of traces, spans, metrics and events ([repo](https://github.com/lmnr-ai/lmnr)). Self-hosting runs PostgreSQL, ClickHouse and Quickwit, and the Helm setup adds RabbitMQ and Redis. Trace ingestion, the SQL editor, dashboards, evaluations, datasets, labeling queues and browser session recording are available without a license; Signals and email or Slack alerts need an enterprise license ([hosting options](https://laminar.sh/docs/hosting-options)).

**Choose Laminar when** you want a purpose-built agent trace UI together with evals, labeling queues and browser session recording, and SQL over your agent traces.

**How Micromegas differs.** The difference is not SQL: Laminar's SQL editor queries spans, traces, evaluation data and datasets, in the ClickHouse dialect, `SELECT` only, with a mandatory time filter and per-project rate limits ([SQL editor](https://laminar.sh/docs/platform/sql-editor)). Micromegas puts agent data in the same store and SQL surface as the rest of your fleet's telemetry (game servers, CI runners, services), and stamps each row with an audience from the ingestion credential. Per-row access control is not described in the Laminar docs read for this page.

## Also considered

- **Helicone**: Apache-2.0, an AI gateway plus observability platform. Self-hosting uses Supabase, ClickHouse and MinIO ([repo](https://github.com/Helicone/helicone)).
- **OpenLLMetry / Traceloop**: Apache-2.0 OpenTelemetry instrumentation for LLM libraries, not a backend. It complements Micromegas, because its output can go to any OTLP backend ([repo](https://github.com/traceloop/openllmetry)); note that Micromegas accepts OTLP over HTTP only.
- **MLflow Tracing**: described as a fully OpenTelemetry-compatible LLM observability solution with native support for GenAI semantic conventions for export and ingestion, and sampling controlled by `MLFLOW_TRACE_SAMPLING_RATIO` ([tracing](https://mlflow.org/docs/latest/genai/tracing/)).

## Using them together

For many teams the realistic answer is both. A purpose-built tool handles evals and the prompt-level UI, while Micromegas keeps the full-resolution, access-controlled record next to the rest of the fleet's telemetry. One setup works with what exists today: send to an OpenTelemetry collector that fans out to both, over a protocol each side accepts. Langfuse, Opik and Micromegas accept OTLP over HTTP, so HTTP is the common denominator. Claude Code selects the protocol with `OTEL_EXPORTER_OTLP_PROTOCOL` ([Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage)), and Micromegas needs `http/protobuf` ([OTLP recipe](../otlp/index.md#claude-code)).

Micromegas ships no integration with the peers above.

## Summary: which one fits

**Choose Micromegas when**:

- you want agent telemetry in the same store and SQL surface as your other telemetry, rather than a separate stack;
- prompts and tool output are sensitive and should be readable only by the person who generated them unless they share them, with metrics visible to a team ([authorization](../admin/authorization.md));
- you want to analyze agent sessions with SQL, notebooks or an agent pointed at the data.

**Choose a purpose-built tool when**:

- you need evals, datasets, experiments, prompt management or a playground (Langfuse, Phoenix, Opik, Laminar);
- you want a trace UI built for LLM calls and agent steps;
- you want no database server at all (Phoenix on SQLite) or the broadest OTLP transport support (Phoenix over gRPC).

## FAQ

### What is a self-hosted alternative to Langfuse that I can query with SQL?

Micromegas is Apache-2.0, self-hosted and queried with SQL over FlightSQL ([query guide](../query-guide/index.md)), but it has no evals or prompt management, so it replaces Langfuse only for recording and querying agent telemetry ([limits](#what-micromegas-records-from-an-agent)). Laminar offers an SQL editor over traces together with evals ([Laminar](#micromegas-vs-laminar)).

### How do I record Claude Code prompts and tool calls without exposing them to my whole team?

Mint a personal audience, set `OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES` and `OTEL_LOG_TOOL_DETAILS`, and send the logs under that key. Rows are readable only by that audience until you add a read grant ([blog post](../blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md), [authorization](../admin/authorization.md)).

### How do I track AI agent token cost per developer or team?

Claude Code exports `claude_code.cost.usage` and `claude_code.token.usage` ([monitoring](https://code.claude.com/docs/en/monitoring-usage)). They land in `measures`, so SQL can group them by user or by the team's resource attributes ([OTLP recipe](../otlp/index.md#claude-code)). Sending metrics under a team-audience key keeps the numbers visible without the prompts ([blog post](../blog/posts/2026-09-03-record-your-ai-agent-share-on-your-terms.md)). Cost is whatever the agent reports; Micromegas has no price table ([limits](#what-micromegas-records-from-an-agent)).

### How do I keep agent telemetry next to the rest of my application telemetry?

Send the agent's OTLP/HTTP output to the same Micromegas ingestion endpoint as your other processes. Agent rows become `log_entries`, `measures` and `otel_spans` beside the rest, queried with the same SQL ([OTLP](../otlp/index.md), [query guide](../query-guide/index.md)).
