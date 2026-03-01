---
date: 2024-08-22
authors:
  - madesroches
categories:
  - Engineering
tags:
  - datafusion
  - lakehouse
  - release
---

# Micromegas v0.1.7 — Lakehouse Architecture

I am proud to release Micromegas 0.1.7.

<!-- more -->

The biggest change is the new lakehouse architecture. Thanks to DataFusion, we can now query millions of log entries or metrics from thousands of processes in a single query.

The new daemon service keeps the materialized views updated on a minute by minute basis.

As before, very high frequency streams where processes send up to 100k events/second remain unprocessed until they are requested — keeping costs as low as ever.

For really scalable Unreal Engine and Rust observability, join the fun :)

- [GitHub](https://github.com/madesroches/micromegas)
- [docs.rs](https://docs.rs/micromegas)
