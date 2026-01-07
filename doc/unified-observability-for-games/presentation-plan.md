# Unified Observability for Game Teams

## Presentation Goal

Convince Unreal Engine game teams to adopt Micromegas by demonstrating the advantages of a unified observability solution over fragmented tooling.

## Target Audience

- Technical Directors at game studios
- Lead Engineers and Engine Programmers
- DevOps/LiveOps teams
- QA leads interested in telemetry

## Core Thesis

**Spend less time reproducing issues by collecting enough data to understand them directly.**

A unified observability stack makes this practical AND powerful:
- **Easier**: One system to learn, one query language (SQL), one place to look
- **More powerful**: Automatic correlation across logs, metrics, and traces - insights you couldn't get from separate tools

---

## Presentation Structure

### 1. Opening: The Goal

**Key message:** Stop reproducing bugs. Collect enough data to understand them directly.

Content:
- The traditional debugging loop: receive report → try to reproduce → fail → ask for more info → repeat
- Reproduction is expensive: hours or days spent recreating conditions
- Some issues can't be reproduced: race conditions, network timing, specific hardware
- **The Micromegas approach**: Collect enough context in production that you can understand issues without reproducing them
- Goal: Know *how often* and *how bad* issues are, with enough detail to *fix* them
- Quantify quality and performance across your entire player base, not just your test machines

### 2. Unification Makes It Easier

**Key message:** One system to learn, one place to look.

Content:
- **One query language**: SQL - everyone already knows it
- **One data model**: Logs, metrics, traces are all timestamped events
- **One retention policy**: 90 days of everything, not different limits per data type
- **Simpler integration**: One SDK to add, one endpoint to configure
- **Less maintenance**: One system to update, monitor, and keep running
- **Team-wide access**: Any engineer can investigate any issue
- No context switching between tools, no exporting data to correlate manually

### 3. Unification Makes It More Powerful

**Key message:** Automatic correlation enables insights separate tools can't provide.

Content:
- **Automatic correlation**: Process ID, thread ID, timestamps link everything without manual effort
- **Cross-data-type queries**: JOIN logs with metrics, filter traces by log conditions
- **Complete context**: Every event knows what else was happening at that moment
- **Questions you can now answer**:
  - "Show me logs from sessions where frame time exceeded 50ms"
  - "What was the CPU trace during this error?"
  - "Compare metrics between sessions that crashed vs sessions that didn't"

Diagram: Side-by-side comparison
- Separate tools: [Logs] → Splunk, [Metrics] → Datadog, [Traces] → ??? (manual correlation)
- Unified: [Logs + Metrics + Traces] → Micromegas → SQL queries (automatic correlation)

### 4. The Game-Specific Advantage

**Key message:** Games have unique observability needs that unified tooling handles better.

Content:
- **Frame-level correlation**: What was happening in the exact frame where the hitch occurred?
- **Client-server correlation**: Same session ID links client trace to server logs
- **Build-to-build comparison**: "Did this commit make things worse?"
- **Map/Level context**: Automatic tracking of which level was loaded
- **High-frequency data**: When you can record thousands of CPU trace events per frame, everything else (logs, metrics, player events) is cheap to record and store

Example scenarios:
1. "Show me all logs from sessions that had >100ms frame times on Map_Desert"
2. "Correlate client FPS drops with server tick rate for the past week"
3. "What percentage of crashes happened during level loading vs gameplay?"

### 5. Player Events

**Key message:** Record everything players do - no limits, no delays, full context.

Content:
- **Unlimited frequency**: Send as many events as bandwidth allows; fast compression built-in
- **Available live**: Events queryable within a minute (or seconds if configured per app)
- **Automatically tagged with game context**: Map, session ID, player ID, build version - all attached without extra code
- Use cases:
  - Player behavior analytics
  - AI/ML training data pipelines
  - Live ops monitoring
  - A/B test analysis
- Same query interface as logs and metrics - no separate analytics system needed

### 6. Unreal Engine Integration

**Key message:** Drop-in integration that enhances what you already have.

Content:
- **Automatic UE_LOG capture**: Zero code changes to get existing logs
- **One header include**: `#include "MicromegasTracing/Macros.h"`
- **Low overhead**: 20ns per cpu event - same as your in-house profiler
- **Familiar patterns**: Similar to Unreal Insights, but with remote storage and SQL queries

Code example:
```cpp
void AMyActor::Tick(float DeltaTime)
{
    MICROMEGAS_SPAN_FUNCTION("Game");  // Trace this function

    // Your existing UE_LOG calls - automatically captured
    UE_LOG(LogGame, Warning, TEXT("Low frame rate"));

    // Add metrics alongside logs
    MICROMEGAS_FMETRIC("Game", Verbosity::Med,
                       TEXT("FrameTime"), TEXT("ms"), DeltaTime * 1000);
}
```

### 7. The Query Power

**Key message:** SQL unlocks insights you couldn't get before.

Content:
- Everyone knows SQL - no proprietary query language to learn
- Join across data types: `SELECT * FROM log_entries l JOIN metrics m ON l.process_id = m.process_id`
- Standard BI tools work: Grafana, Python, any SQL client
- Example queries:
  - "Average frame time by map, grouped by build version"
  - "Logs from sessions where any metric exceeded threshold X"
  - "Trace events correlated with error logs"

### 8. Cost Reality Check

**Key message:** Unified doesn't mean expensive - quite the opposite.

Content:
- Real production numbers:
  - 449 billion events over 90 days
  - ~$1,000/month total (compute + storage + database)
  - That's logs + metrics + high-frequency traces combined

### 9. Architecture Overview

**Key message:** Simple, proven components.

Content:
- Ingestion: HTTP service → PostgreSQL (metadata) + S3/GCS (payloads)
- Query: Apache Arrow FlightSQL + DataFusion
- Visualization: Grafana plugin, Python API, or Perfetto export
- Scale: Handles 266M events/minute peaks

Diagram: Mermaid flow chart showing data path

### 10. Getting Started Path

**Key message:** Start small, prove value, expand.

Content:
1. **Week 1**: Set up cloud infrastructure in a dedicated account (I can help - we have a template)
2. **Week 2**: Install plugin - logs, metrics, and CPU traces available right away
3. **Week 3**: Start from our Grafana dashboards, make them your own
5. **Ongoing**: Expand instrumentation in game-specific code based on what you learn

### 11. Closing: Fix More, Reproduce Less

**Key message:** Understand issues from the data, not from reproduction.

Content:
- Stop the "reproduce → fail → ask for more info" loop
- Know how often issues happen and how severe they are - across all players
- **Easier**: One query, one system, anyone on the team can investigate
- **More powerful**: Complete context - logs, metrics, traces correlated automatically
- Your instrumentation investment compounds over time - each event adds context

Call to action:
- GitHub: github.com/madesroches/micromegas
- Documentation: [link]
- Contact for initial setup help

---

## Key Talking Points to Emphasize

1. **"No reproduction required"** - Collect enough data to understand issues directly
2. **"How often and how bad"** - Quantify issues across your entire player base
3. **"Always-on profiling"** - 20ns overhead means you can profile in production, not just during debugging
4. **"No remote access needed"** - Data streams automatically; no attaching debuggers or downloading files from remote PCs/VMs
5. **"No mess"** - All observability managed centrally; no log files and captures shared via email or Teams
6. **"Share by URL"** - Easily share and record discoveries with a link; no screenshots or descriptions needed
7. **"Data available live"** - Player events and AI training data queryable within a minute (or seconds if configured)
8. **"Real MTBF"** - Track stability properly; crash databases only know crashes, Micromegas knows when apps run fine too
9. **"Easier"** - One system, one query language, one place to look
10. **"More powerful"** - Automatic correlation enables queries you couldn't do before
11. **"Easier AND more powerful"** - Usually you trade one for the other; unification gives you both

## Potential Objections to Address

| Objection | Response |
|-----------|----------|
| "Self-hosted is too much work" | PostgreSQL + S3 are well-understood. One deployment handles everything. |
| "Our data is sensitive" | Automated retirement of data |
| "We need real-time alerting" | Daemon processes low-frequency streams. Grafana alerting works. |

## Visual Assets Needed

1. **Comparison diagram**: Fragmented vs Unified tooling
2. **Architecture diagram**: Ingestion and query flow (Mermaid)
3. **Screenshot**: Grafana dashboard with logs+metrics
4. **Screenshot**: Perfetto trace visualization
5. **Code snippets**: Unreal integration examples
6. **Cost comparison table**: Micromegas vs vendor pricing

## Timing Estimate

| Section | Duration |
|---------|----------|
| 1. Opening: The Goal | 3 min |
| 2. Unification Makes It Easier | 3 min |
| 3. Unification Makes It More Powerful | 3 min |
| 4. Game-Specific Advantage | 4 min |
| 5. Player Events | 3 min |
| 6. Unreal Engine Integration | 4 min |
| 7. The Query Power | 4 min |
| 8. Cost Reality Check | 3 min |
| 9. Architecture Overview | 3 min |
| 10. Getting Started Path | 2 min |
| 11. Closing | 2 min |
| **Total** | ~34 min |

Buffer for Q&A: 10-15 minutes

---

## Notes for Speaker

- Lead with the goal everyone wants: stop wasting time on reproduction
- "How often and how bad" resonates - game teams care about quantifying quality
- Hammer the theme: **easier AND more powerful** - unification gives you both
- "Easier" examples: one query language, one system to learn, one place to look
- "More powerful" examples: queries that JOIN logs with metrics, automatic correlation
- Show concrete examples of queries that would have required reproduction before
- The cost numbers are real and compelling - comprehensive data collection is affordable
- Demo the query interface if possible (live or video)
- Emphasize the "aha moment": one query shows logs AND metrics AND traces from the same session
- Open source framing - sharing a solution that works, not selling
