---
date: 2024-04-27
authors:
  - madesroches
categories:
  - Engineering
tags:
  - architecture
  - cost
  - arrow
  - parquet
  - datafusion
---

# How to Record Millions of Events for Pennies

How to record millions of events for pennies.

<!-- more -->

- Forget JSON, use a compact binary format.
- Store the data in S3, index it in PostgreSQL.
- Delay deep indexing as much as possible.
- You can probably delete it before you ever need to take a look.
- Leverage Arrow, Parquet, and DataFusion.

[Work in progress on GitHub](https://github.com/madesroches/micromegas)
