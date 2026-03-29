---
date: 2026-03-29
authors:
  - madesroches
categories:
  - Opinion
tags:
  - observability
  - ai
  - candor
---

# From Observability to Candor

You don't need a complex system to interrogate your infrastructure.

<!-- more -->

## The gold rush is overengineered

Everyone's racing to ship AI-powered ops right now. Fine-tuned anomaly detectors. RAG pipelines over logs. Custom embeddings for trace similarity. MCP servers to expose metrics to LLMs. It's a lot of machinery.

Custom models need labeled data and retraining cycles. RAG needs chunking strategies and vector stores. MCP means building and maintaining a protocol server — when a CLI that returns text does the same job with zero integration overhead.

Most of this is unnecessary. The complexity is in the integration, not the intelligence. We've been treating "AI for ops" as a modeling problem when it's actually a data problem. The companies shipping the most impressive AI ops demos aren't the ones with the best models — they're the ones with the cleanest data pipelines.

## Foundation models changed the equation

Large language models already know what a stack trace is. They can parse log lines, reason about latency distributions, and follow a span hierarchy through a distributed system. No training. No fine-tuning. No labeled datasets. They just read.

The missing piece was never intelligence — it was unified telemetry.

Most organizations have logs in one tool, metrics in another, traces in a third. Three query languages, three mental models, three vendor APIs. No single prompt can bridge that fragmentation. You can give a foundation model access to Datadog *and* Splunk *and* Jaeger, but you're still asking it to juggle three incompatible worldviews.

Unify the telemetry, give it a SQL interface, and the model does the rest.

## Unified observability is the real work

Merge logs, metrics, and traces into one SQL-queryable data lake and you've done 90% of the work. One store. One query language. One interface.

This is what we built with [Micromegas](https://github.com/madesroches/micromegas): a unified observability platform that stores everything — logs, metrics, spans — in a single lakehouse architecture. Metadata in PostgreSQL, payloads in cheap object storage, queries via Apache Arrow FlightSQL.

The CLI is `micromegas-query`. SQL in, structured data out. That's the entire API surface a foundation model needs. No MCP server, no custom API, no adapter layer. A CLI that a human can use is a CLI a model can use. The interface is plain text — the one format every foundation model already handles natively.

## A markdown file, not a model

The "AI integration" is a skill file — a markdown document describing the schema, safety rules, and common query patterns. About 350 lines. No code. No embeddings. No vector store. No RAG pipeline.

It's the same documentation you'd write for a new on-call engineer: here are the tables, here's how they relate, here's what's expensive to scan, here are the queries that answer the questions people actually ask.

We wrote a Claude Code skill, but the principle applies to any capable foundation model. Give it documentation and a CLI, and it figures out the rest. The model reads the schema, writes SQL, runs the query, interprets the results, and tells you what's going on.

You ask "what errors spiked in the last hour?" and get a straight answer — not a dashboard to squint at. Ask a follow-up — "show me the stack traces for those errors" — and it writes the next query, correlates the data, and explains what it found. The conversation *is* the investigation.

## Candor

Observability is passive. You look at dashboards. You set up alerts. You wait for something to turn red, then you start investigating.

Candor is active. You ask questions and get answers. You have a conversation with your running systems, and they tell you what's actually happening.

Systems that tell it like it is.

If your data is unified and queryable, the foundation model is the last mile. And it's already built. Customize a skill file with your schemas — which tables are cheap to scan, which need tight time ranges, how to pivot from process metadata down to logs or spans. That's the whole integration. A markdown file and a CLI.

The tooling for candor already exists. The question is whether your telemetry is ready for it.
