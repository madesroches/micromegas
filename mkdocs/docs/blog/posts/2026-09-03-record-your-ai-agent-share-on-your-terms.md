---
date: 2026-09-03
authors:
  - madesroches
categories:
  - Engineering
tags:
  - ai
  - claude-code
  - access-control
  - privacy
  - security
  - candor
  - observability
---

# Record Everything Your AI Agent Does. Share It On Your Terms.

A coding agent running on your laptop is the most revealing process you own. Every prompt you typed, every file it read, every shell command it ran, every secret that scrolled past in a tool result. That is exactly the telemetry you want when the agent goes sideways — and exactly the telemetry nobody dares to ship to a shared observability backend.

Micromegas now lets you record the whole thing and decide, per person and per team, who gets to read it. The data never moves when you change your mind.

<!-- more -->

## The fearless-recording problem

Claude Code emits a rich Open Telemetry surface, and it redacts the interesting parts by default. Prompt text, assistant responses, the actual `bash` command a tool ran, tool inputs and outputs — each sits behind its own opt-in flag (`OTEL_LOG_USER_PROMPTS`, `OTEL_LOG_ASSISTANT_RESPONSES`, `OTEL_LOG_TOOL_DETAILS`, `OTEL_LOG_TOOL_CONTENT`). Anthropic is right to ship them off. A conversation is where the API token you pasted "just for a second" lives, where the customer's stack trace with their email in it lives, where `env | grep KEY` output lives.

The traditional observability model makes that a binary choice. Everything ingested lands in one index, and everyone with a dashboard login can search it. So you either flip the flags on and hope, or you leave them off and debug your agent from `tool_name=Bash success=false duration_ms=412`. Most teams choose the second, and the recording is useless precisely when you need it.

The way out is not better redaction. It is to make *who can read a row* a property of the row itself, decided by the credential that wrote it, and to make *sharing* a separate, cheap, reversible decision.

## What it looks like from the laptop

One command mints you a personal ingestion key and claims an audience nobody else can read:

```bash
eval "$(micromegas-setup-telemetry --url https://analytics.example.com \
    --name my-laptop --claim "$USER-claude")"

export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=otlp OTEL_LOGS_EXPORTER=otlp
export OTEL_LOG_USER_PROMPTS=1
export OTEL_LOG_ASSISTANT_RESPONSES=1
export OTEL_LOG_TOOL_DETAILS=1
claude
```

The claim is self-service: the operator flips `MICROMEGAS_SELF_SERVICE_MINT` on once, and from there no admin ticket stands between a developer and a private audience.

From then on every prompt, response, tool call, API request, and cost figure lands in the lakehouse as ordinary `log_entries` and `measures` rows, next to your game servers and CI runners. Query them with `micromegas-query`, or point Claude at them and ask what went wrong in yesterday's session. Nobody else in the org sees a byte of it.

When you *do* want to share — a bug report where a colleague needs the full transcript, a team lead reviewing how the agents are being used, a retro on a costly runaway loop — you open the Audience Access page and add a read grant for a user or a group. It applies to everything you have already recorded, within a minute. Delete the grant and it stops. Nothing is copied, rewritten, or re-ingested.

## Self-service cost optimization, no creepiness required

The first thing this unlocks is not surveillance. It is the opposite: letting people fix their own agent usage.

Agent bills are driven by things only the full record reveals. Which sessions burned cache-creation tokens because a huge file got re-read every turn. Which skill triggers a ten-minute subagent fan-out for a one-line question. Which repeated prompt pattern could be a saved command. The `api_request` events carry model, effort, cost, and token counts per call; the `tool_result` events say what ran and how long it took; the prompts say why. Join them and the waste is obvious. But the *why* is the sensitive part, and nobody should have to hand it to a manager to find out they are spending forty dollars a day re-reading `Cargo.lock`.

With a private audience, nobody has to. Point Claude at your own data and ask. The integration is [a skill file and a CLI](2026-03-29-from-o11y-to-candor.md) — a markdown document describing the tables, handed to the same agent that produced the data. *"Which of my sessions this week cost the most, and what were they doing?"* comes back with the sessions, the tool loops that ran up the bill, and a suggestion. The developer optimizes their own workflow against their own transcript, and the transcript never leaves their audience.

Management still gets the numbers, without the words. Claude Code exports metrics and logs as separate signals, and honors per-signal OTLP headers, so a laptop can send `claude_code.cost.usage` and `claude_code.token.usage` under a key bound to a team audience while the prompt and tool events go under the personal one:

```bash
# metrics: team-readable, no prompt content by construction
export OTEL_EXPORTER_OTLP_METRICS_HEADERS="Authorization=Bearer <team-audience-key>"
# logs: personal audience, prompts and tool details included
export OTEL_EXPORTER_OTLP_LOGS_HEADERS="Authorization=Bearer <personal-audience-key>"
```

Metrics carry counts, costs, tokens, and durations, never text. A finance dashboard over `measures` filtered to the team audience answers "what does the platform group spend on agents, by model" precisely, and cannot answer "what did Alice ask it on Tuesday." The boundary is not a redaction rule someone has to maintain. It is which key wrote the row.

## How it works

### The stamp is immutable

Three Postgres tables anchor everything in Micromegas: `processes`, `streams`, and `blocks`. Each row now carries an `audience` column, written at insert time from the *authenticated ingestion credential*, never from the payload. An ingestion key bound to `alice-claude` cannot write a row labelled anything else, and there is no `UPDATE` path that changes a stamp afterward.

Every row carries its own stamp rather than inheriting one from the process it belongs to, so a block that *claims* a foreign `process_id` cannot borrow that process's label. It carries the audience of whoever wrote it, and at materialization a single predicate on the `blocks` view drops any block whose stamp disagrees with the stream and process it points at. Every downstream view — `log_entries`, `measures`, `log_stats`, the per-process span views — reads blocks from those materialized partitions, so the check lives in one place and is inherited everywhere for free.

Immutability is what makes the rest cheap. The label is a physical, dictionary-encoded column in Parquet files sitting in object storage. If sharing meant relabelling, sharing would mean rewriting terabytes. It doesn't.

### Sharing is a grants edit, never a restamp

Who may read `alice-claude` is not encoded in the data. It is a handful of rows in a separate `audience_grants` table, each an `(audience, axis, selector)` triple:

| audience | axis | selector |
|---|---|---|
| `alice-claude` | `read` | `user:alice@example.com` |
| `alice-claude` | `mint` | `user:alice@example.com` |
| `alice-claude` | `read` | `group:agent-platform` |

The first two rows were written by the `--claim` above. The third is Alice sharing with a team. Selectors are `*`, `user:<email>`, or `group:<name>`, with groups nested transitively in a `groups` table of their own. A non-admin can share what they hold with a user or a group, never with `*`; they can revoke what they created, or remove their own access. Admins see everything and can do everything. All of it is auditable from SQL through `list_audience_grants()`, and every row records who created it and when.

Each query server holds a whole-table snapshot of the grants, refreshed every 60 seconds. That is the entire cost of authorization state: a small table, one cached read.

### Row filtering as a plain predicate

At query time an authenticated session gets a `ReadScope` naming its audiences, and a DataFusion `AnalyzerRule` called `OwnershipRewrite` walks every plan. For the six views that carry the `audience` column — `processes`, `streams`, `blocks`, `log_entries`, `measures`, `log_stats` — it injects one filter:

```sql
audience IN ('alice-claude', 'public', 'team-alpha')
```

No join, no subquery, no per-row lookup. Because `audience` is a real column, the filter is pushed down to the Parquet scan like any other predicate. Row groups whose statistics cannot match are skipped without being read.

This is the part worth dwelling on, because it inverts the usual deal. Enterprise authorization is almost always a tax: a post-filter over rows the engine already fetched, a policy engine consulted per row, or a join against an entitlements table that grows with the org. Access control makes every query slower, so it gets deployed grudgingly and scoped as narrowly as legal will allow. Here the filter lands *before* the I/O. Every block comes from one process under one credential, so rows arrive in long runs of a single audience, and a row group whose min/max statistics exclude the caller's audiences is never fetched. A user scoped to their own agent's traffic reads only the bytes that hold that traffic. Tighter authorization means fewer bytes off object storage. A colleague with one audience gets the fast path; the admin who reads everything pays the same price as before, never more.

The rule is keyed on the view's schema, not on a hardcoded list, and it fails closed: a view set the rule does not recognise makes the query error rather than silently returning unfiltered rows.

### `view_instance` gating for the views that have no column

Not every view needs the column. `thread_spans`, `async_events`, `otel_spans`, `net_spans`, and `images` are materialized just-in-time per process or per stream, and are only reachable through `view_instance(view_set, instance_id)`. An instance belongs to exactly one process or stream, and that process or stream is stamped with exactly one audience. So every row a `view_instance` scan can return has the same answer to "who may read this," and it is known before the scan starts.

That is a property worth exploiting rather than ignoring. Instead of filtering rows, `AudienceGuard` resolves the instance id with a single primary-key point query against Postgres — `SELECT audience FROM processes WHERE process_id = $1`, or the same on `streams` — cached in memory, and either admits the whole scan or refuses it. No predicate is injected for these five views at all. The Parquet reader runs exactly as it would with authentication off.

A denial and a nonexistent id produce the same `not found or not accessible` error, so a caller cannot use the guard to enumerate other people's process ids. And because the check runs before the JIT materialization step, a caller cannot make the server spend compute and object-storage writes materializing someone else's process, only to be handed zero rows afterward.

Two mechanisms, one principle: when the answer is per row, filter per row on a physical column and let pushdown do the work. When the answer is per scan, decide once and leave the scan alone.

The same guard fronts the arg-addressed functions that bake a target id into the plan — `process_spans`, `perfetto_trace_chunks`, `parse_block`, `get_payload` — and `list_partitions()` quietly omits partitions that are not yours.

## The legal angle

*Not legal advice; talk to your privacy officer.* But the shape of the obligation is the same in most places your employees live, and the point is that this architecture happens to fit it.

A transcript of what a person typed into an assistant, and what it did on their machine, is personal data about an identifiable employee. In the EU, the GDPR's data-minimisation and purpose-limitation principles mean an employer may keep only what is necessary for a stated purpose and expose it only to those who need it; systematic monitoring of staff calls for a Data Protection Impact Assessment and a proportionality analysis, and employees keep a right of access to what was collected about them. Quebec's Law 25 requires the highest confidentiality settings *by default* and a privacy impact assessment before rolling out a new monitoring system. California's CPRA has covered employee data since 2023: notice at collection, and rights to know and to delete.

Read those as engineering requirements and they say: private by default, need-to-know sharing, a record of who could see what, and a way for the person to see their own data. That is what falls out of audiences:

- **Private by default.** A self-claimed audience is readable by its owner and nobody else. There is no "everyone can see it until we lock it down" phase.
- **Need-to-know, decided by the data subject.** The person who generated the transcript is the one who grants a colleague or a team access to it, scoped to their own data, and can withdraw it.
- **An audit trail that is the access control.** `audience_grants` is not a log *about* permissions; it is the permissions. `list_audience_grants()` answers "who could read Alice's sessions in March" from the same SQL prompt everything else uses.
- **Right of access, built in.** An employee asking what was recorded about them runs `SELECT * FROM log_entries` under their own identity and gets exactly the rows stamped with their audiences.
- **Retention that deletes.** Blocks age out of Postgres and object storage on the configured schedule, and the guard denies access the moment the metadata row is gone.

None of this makes a monitoring program lawful on its own. It does mean the technical controls a privacy assessment will ask for are already there, instead of a project you have to build before you can turn the recording on.

## Turn the flags on

The trade every team has been making — record nothing useful, or record everything into a shared index — was never about telemetry. It was about not having a per-row answer to "who is allowed to read this." Now there is one: stamped by the credential that wrote the row, enforced as a plain Parquet predicate or a single point lookup, and shared by editing a table instead of moving data.

Set `OTEL_LOG_USER_PROMPTS=1`. Your colleagues will not see it unless you say so.

---

Audiences, grants, and the enforcement layers are documented in [Authorization](../../admin/authorization.md); groups in [Groups](../../admin/groups.md); the Claude Code recipe in the [OTLP docs](../../otlp/index.md#claude-code). Source is on [GitHub](https://github.com/madesroches/micromegas).
