# Unified Observability for Game Teams

## The Pitch

**Stop reproducing bugs. Collect enough data to understand them directly.**

A unified observability stack makes this practical:
- **Easier**: One system to learn, one query language, one place to look
- **More powerful**: Automatic correlation unlocks insights fragmented tools can't provide

## Target Audience

- Technical Directors and Engineering leads evaluating observability strategy
- Engine programmers tired of debugging with insufficient data
- Teams maintaining multiple internal telemetry systems

## The Problem

**Fragmentation.** Most studios have:
- A logging system
- A metrics pipeline
- A profiling solution
- A player analytics system

Each built independently. Each solving one piece. None talking to each other.

**This presentation argues:** Unification beats fragmentation. It's easier AND more powerful.

---

## Presentation Structure

### 1. The Pain (3 min)

**Open with what they already know hurts.**

The debugging loop everyone hates:
1. Receive bug report: "Game hitched during combat"
2. Try to reproduce → fail
3. Ask for more details → wait
4. Try again → can't replicate exact conditions
5. Mark as "cannot reproduce" or waste days guessing

The hard truth:
- Some bugs **can't** be reproduced: race conditions, specific hardware, network timing
- You only know about **reported** issues - how many players quit silently?
- Test machines aren't production

**The question to plant:** What if you could understand issues without reproducing them?

---

### 2. The Goal: Reproduce Less, Fix More (3 min)

**Reframe the problem.**

Instead of reproducing issues, collect enough data to **understand them directly**:
- Know **how often** issues happen across your entire player base
- Know **how bad** they are: severity, duration, impact
- Have **enough context** to fix them without guessing

This requires:
- Logs (what happened)
- Metrics (how bad)
- Traces (why it happened)
- All correlated, all queryable

**The question:** Do you build this as separate systems, or as one?

> **Slide visual:** Screenshot 4 (Metrics + Property Timeline) - preview the payoff, show "how often and how bad" in one view

---

### 3. The Fragmented Reality (4 min)

**Acknowledge what most studios have.**

Typical internal tooling landscape:
- **Logs**: Some system - maybe custom, maybe files on disk
- **Metrics**: Some system - probably different from logs
- **Traces**: Local profiling, or nothing for remote sessions
- **Player events**: Separate pipeline, often a different team

Each system was built to solve a specific problem. Each works in isolation.

**The friction this creates:**

| Task | Fragmented Reality |
|------|-------------------|
| "I got disconnected!" | Hunt through local files, find the right server VM, manually correlate timestamps |
| Correlate client hitch with server state | Hope timestamps align, manual cross-reference |
| Compare crash rate by build | Query crash DB, query metrics DB, join in spreadsheet |
| Debug a player-reported issue | Check 3-4 different tools, piece together the story |

Every investigation requires **manual correlation**. Context lives in your head, not in the tools.

> **Slide visual:** Diagram - "Fragmented vs Unified" showing N separate tools vs one unified system

---

### 4. The Case for Unification: Easier (4 min)

**Key message:** One system beats three.

**One query language**
- SQL. Everyone knows it (or can learn it - AI knows it too and can teach you).
- No custom DSL per tool. No learning curve per system.
- Any engineer can investigate any issue.

**One data model**
- Logs, metrics, traces, player events - all timestamped events
- Same schema concepts across data types
- Single retention policy to manage

**One place to look**
- Stop asking "which tool has this data?"
- Stop context-switching between UIs
- Stop maintaining tribal knowledge of "use tool X for problem Y"

**One integration**
- One SDK to add to your game
- One endpoint to configure
- One system to update and monitor

**The maintenance argument:**
- You're already maintaining multiple internal tools
- Each has its own bugs, its own backlog, its own experts
- Unification means one system to improve, not three to keep alive

> **Slide visuals:**
> - Screenshot 1 (Processes List) - "one place to look" - all processes, searchable
> - Screenshot 2 (Process Overview) - "one data model" - logs, metrics, traces counted together

---

### 5. The Case for Unification: More Powerful (4 min)

**Key message:** Automatic correlation unlocks questions you couldn't ask before.

**Why fragmented tools can't do this:**
- Each tool has its own identifiers
- Timestamps might not align precisely
- No shared context (session, map, build) across systems
- Correlation requires export → manual join → hope it works

**What unification enables:**

Every event automatically shares:
- Process ID, thread ID
- Session ID, player ID
- Map/level, build version
- Precise timestamps

Queries that become trivial:
- "Show me logs from sessions where matchmaking time exceeded 30s"
- "What was the server doing when the client hitched?"
- "Crash rate by map and build version"
- "All events from this player's session, sorted by time"

**With fragmented tools:** Each of these is a multi-hour investigation across multiple systems.
**With unified data:** Each is a single query.

> **Slide visuals:**
> - Screenshot 3 (Log Viewer with Search) - queryable logs with filtering, SQL visible
> - Screenshot 4 (Metrics + Property Timeline) - THE KEY SLIDE - metrics correlated with game context

---

### 6. Game-Specific Power (4 min)

**Key message:** Games have unique needs. Unification handles them better than fragmented tools.

**Frame-level precision**
- What happened in the exact frame where the hitch occurred?
- Unified: CPU trace + logs + metrics, same timestamp, one query
- Fragmented: Hope your profiler was running, cross-reference manually, find the dumped files on the user's machine

**Client-server correlation**
- Session ID links client and server automatically
- Debug desyncs by seeing both perspectives in one query
- Fragmented: Different logging systems, different session concepts, manual alignment

**Build-to-build comparison**
- "Did this commit make things worse?" - one query
- Fragmented: Export from metrics system, hope build tags are consistent

**Map/Level context**
- Automatic tagging of which level was loaded
- "Show me hitches on Map_Desert" - one query
- Fragmented: Did you remember to tag that in every system?

> **Slide visuals:**
> - Screenshot 5 (Performance Analysis) - thread coverage timeline, trace event count, "Open in Perfetto" button
> - Screenshot 6 (Perfetto Trace) - flame graph view, multiple threads, function names - the deep dive

---

### 7. Player Events: Same System, Full Context (3 min)

**Key message:** Player analytics shouldn't be a separate island.

The fragmented pattern:
- Engineering telemetry: one system
- Player analytics: different team, different pipeline, different tools
- Correlating player behavior with technical issues: manual, slow, often impossible

The unified approach:
- Player events flow through the same pipeline
- Same query interface as logs and metrics
- Automatic context: session, map, build, player ID

What this enables:
- "Players who experienced hitches - what were they doing?"
- "A/B test results with performance breakdown by group"
- "Correlate player progression with crash frequency"

No separate analytics system. No export/import. One query interface.

---

### 8. Unreal Integration (4 min)

**Key message:** Drop-in, low overhead, enhances what you have.

**Zero-code start:**
- Install plugin
- UE_LOG calls automatically captured
- Logs flowing within minutes

**Add instrumentation where it matters:**
```cpp
#include "MicromegasTracing/Macros.h"

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

**Performance:**
- 20ns per CPU trace event
- Always-on profiling is practical
- Profile production, not just debug builds

**Compared to local profiling:**
- Local profilers are great for local debugging
- This adds: remote collection, SQL queries, correlation with logs/metrics
- They complement each other

---

### 9. SQL: One Language to Learn (3 min)

**Key message:** One query language beats N proprietary ones.

**The problem with fragmented tools:**
- Each tool has its own query syntax
- PromQL, custom DSLs, proprietary languages
- Every new system means another language to learn

**Why SQL:**
- One language for all data types
- 50 years old, not going anywhere
- Easy to pick up - AI can write queries for you and explain them
- Massive ecosystem of tutorials, tools, and expertise

**Standard tooling works:**
- Grafana dashboards
- Python notebooks
- Easy to export data to your existing BI tools

**Query power:**
- "Average frame time by map and build"
- "Trace events in the second before and after each kill"
- "Sessions that crashed vs sessions that didn't - what's different?"

> **Slide visual:** Screenshot 7 (SQL Query Editor) - real SQL with JOINs, variable values, Execute button

---

### 10. Interoperability: Unified, Not Isolated (3 min)

**Key message:** Unified doesn't mean closed. Standard protocols make integration easy.

**Export what you need**
- Python API for programmatic access
- Tail-sample high-frequency data, export the subset you care about
- Feed into ML pipelines, external dashboards, long-term archives

**Standard protocols**
- Analytics service speaks FlightSQL - an open standard
- We have existing FlightSQL clients : Python, Grafana (Go), Rust
- No proprietary lock-in

**Query federation**
- Plug other systems into the same query layer
- Join your unified data with external sources
- The protocol is the integration point, not custom connectors

**Why this matters:**
- You don't have to move everything at once
- Existing systems can pull data they need
- New systems can be added without changing the core

> **Slide visual:** Screenshot 8 (Grafana Dashboard) - standard tooling, multiple panels, same data

---

### 11. Getting Started (2 min)

**Key message:** Start alongside your existing tools. Prove value. Expand.

**Week 1**: Stand up infrastructure (Terraform templates available) - I can help.
**Week 2**: Install plugin, see first data in Grafana and the analytics web app
**Week 3**: Teach all teams how to get the data they need and how to make their own reports & dashboards.
**Ongoing**: Expand instrumentation based on what you learn

You don't have to rip out your existing tools. Run Micromegas alongside them. When it proves value, migrate.

---

### 12. Closing (2 min)

**The core argument:**

| Fragmented | Unified |
|------------|---------|
| Multiple tools to learn | One query language |
| Manual correlation | Automatic correlation |
| Context in your head | Context in the data |
| Each system maintained separately | One system to improve |
| "Cannot reproduce" | "Here's what happened" |
| "Where is the log?" | "I can see others are having this issue" |

**The thesis, restated:**

Unified observability is **easier** (one system, one language, one place) AND **more powerful** (automatic correlation, queries you couldn't run before).

You don't usually get both. Unification gives you both.

**Call to action:**
- GitHub: [github.com/madesroches/micromegas](https://github.com/madesroches/micromegas)
- Happy to help with initial setup and infra management

**Final message:** Reproduce less. Fix more. Ship better games.

> **Slide visual:** Diagram - "Fragmented vs Unified" (same as Section 3) - reinforce the core message

---

## Objection Handling

| Objection | Response |
|-----------|----------|
| "Our tools work fine" | The opportunity is in correlation - that's where unified data shines. |
| "Migrating is too risky" | Run alongside what you have. Prove value first. Migrate incrementally. |
| "We don't have time to set this up" | Compare to the time spent maintaining multiple separate systems. Initial setup is days, not months. |
| "Our data is sensitive" | Self-hosted. Your cloud, your data. No third-party access. |
| "We need features X, Y, Z" | What features? Often they're already there. If not, it's open source - we can add them. |
| "What about local profiling?" | Local profiling is great for local debugging. This adds remote collection and correlation. They complement each other. |
| "Our scale is different" | 266M events/minute, 449B events stored. What's your scale? |
| "What if this project dies?" | Open source, standard formats. No lock-in. |

---

## Key Phrases

Hammer these throughout:

1. **"Easier AND more powerful"** - The core thesis. Unification gives you both.
2. **"Reproduce less, fix more"** - The promise.
3. **"One query, complete context"** - Unification payoff in five words.
4. **"How often and how bad"** - Quantify issues, don't just report them.
5. **"Automatic correlation"** - What fragmented tools can't do.
6. **"Your instrumentation compounds"** - Every event adds context to every other.

---

## Visual Assets

### Diagrams to Create

| Asset | Section | Purpose |
|-------|---------|---------|
| Fragmented vs Unified diagram | 3, 12 | Core argument - show N tools vs one |

### Screenshots from Analytics Web App

Capture these from the analytics web app running with Micromegas service telemetry (dogfooding):

#### Screenshot 1: Processes List (Section 4 - "One place to look")
- **Page**: `/processes`
- **Show**: List of processes (ingestion-srv, flight-sql-srv, etc.) with exe names, computers, timestamps
- **Why**: Demonstrates "one place to look" - all your processes, searchable
- **Capture tips**: Include search bar, show variety of service names

#### Screenshot 2: Process Overview (Section 4 - "One data model")
- **Page**: `/process` (detail view)
- **Show**: The 5-panel info card layout:
  - Process info (exe, PID)
  - Environment (computer, CPU)
  - Timing (start, duration)
  - Statistics (log count, metrics count, trace events, threads)
  - Properties
- **Why**: Shows unified metadata - logs, metrics, traces all counted together
- **Capture tips**: Pick a process with good numbers in all stats

#### Screenshot 3: Log Viewer with Search (Section 5 - "Automatic correlation")
- **Page**: `/process_log`
- **Show**:
  - Log entries table with timestamp, level, target, message
  - Search filter active (e.g., searching for "hitch" or "error")
  - Log level dropdown showing filtering options
  - SQL query visible in right panel
- **Why**: Shows queryable logs with filtering - "one query, complete context"
- **Capture tips**: Search for something like "error" or "query", show mixed log levels

#### Screenshot 4: Metrics Chart with Property Timeline (Section 5 & 6 - "More Powerful" & "Game-Specific")
- **Page**: `/process_metrics`
- **Show**:
  - Time series chart of a metric (query latency, request count, memory, etc.)
  - Property timeline below showing state changes
  - P99/Max toggle visible
  - Metric selector dropdown
- **Why**: This is THE money shot - metrics correlated with context automatically
- **Capture tips**:
  - Pick a metric with visible variation (query latency works well)
  - Enable property timeline if available
  - Zoom to an interesting time range

#### Screenshot 5: Performance Analysis with Thread Coverage (Section 6 - "Frame-level precision")
- **Page**: `/performance_analysis`
- **Show**:
  - Metrics chart at top
  - Thread coverage timeline showing multiple threads
  - Trace event count visible
  - "Open in Perfetto" button visible
- **Why**: Shows frame-level analysis capability, thread visibility
- **Capture tips**: Select a time range with interesting thread activity

#### Screenshot 6: Perfetto Trace View (Section 6 - "Frame-level precision")
- **Source**: Perfetto UI (ui.perfetto.dev) after clicking "Open in Perfetto"
- **Show**:
  - Flame graph / trace view
  - Multiple threads visible
  - Span names from game code
- **Why**: Shows the deep-dive capability - when you need frame-level detail
- **Capture tips**: Zoom to show a few interesting frames with clear function names

#### Screenshot 7: SQL Query Editor (Section 9 - "SQL: One Language")
- **Page**: Any page, focus on right panel
- **Show**:
  - SQL query with JOINs or interesting WHERE clauses
  - Variable values displayed
  - Execute button
- **Why**: Reinforces "one query language" - real SQL, not proprietary DSL
- **Capture tips**: Show a query that spans multiple data types if possible

#### Screenshot 8: Grafana Dashboard (Section 10 - "Interoperability")
- **Source**: Grafana with Micromegas datasource
- **Show**:
  - Dashboard with multiple panels
  - Mix of metrics and logs if possible
  - Time range selector
- **Why**: Shows standard tooling works - not a closed ecosystem
- **Capture tips**: Create a service-focused dashboard (request latency, error rates, throughput)

### Capture Checklist

Before capturing:
- [ ] Use Micromegas's own telemetry data (services dogfooding themselves) - it's public and shows real-world usage
- [ ] Use dark theme if available (better for presentations)
- [ ] Browser at consistent width (1920px recommended)
- [ ] Hide browser chrome/bookmarks bar

---

## Timing

| Section | Duration |
|---------|----------|
| 1. The Pain | 3 min |
| 2. The Goal | 3 min |
| 3. Fragmented Reality | 4 min |
| 4. Easier | 4 min |
| 5. More Powerful | 4 min |
| 6. Game-Specific | 4 min |
| 7. Player Events | 3 min |
| 8. Unreal Integration | 4 min |
| 9. SQL | 3 min |
| 10. Interoperability | 3 min |
| 11. Getting Started | 2 min |
| 12. Closing | 2 min |
| **Total** | **39 min** |

Buffer for Q&A: 20 min. Fits a 60 min slot comfortably.

---

## Speaker Notes

**The argument structure:**
1. Pain (they nod along)
2. Goal (reframe the problem)
3. Fragmented reality (acknowledge what they have)
4. Easier (first half of thesis)
5. More powerful (second half of thesis)
6. Proof points (game-specific, player events, Unreal integration)
7. Interoperability (unified doesn't mean isolated)
8. Practical path forward

**Key moment:** Section 5, the SQL queries. Show queries that would take hours with fragmented tools, done in one line. That's the "aha."

**Don't:**
- Bash their existing tools (they built them, they're proud of them)
- Oversell - the real numbers are compelling enough
- Get lost in architecture details

**Do:**
- Acknowledge the investment already made - people built those systems, they work
- Position as evolution, not replacement
- Show real queries that answer real questions
- Use their language: "frames," "hitches," "desyncs," "builds"
- Pause on Screenshot 4 (metrics + property timeline) - this is the "aha" moment
- Let the Perfetto screenshot sink in - familiar tool, remote data
