# Unified Observability for Game Teams - Speaker Guide

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

---

## Speaker Notes

**Don't:**
- Bash their existing tools (they built them, they're proud of them)
- Oversell - the real numbers are compelling enough
- Get lost in architecture details

**Do:**
- Acknowledge the investment already made - people built those systems, they work
- Position as evolution, not replacement
- Show real queries that answer real questions
- Use their language: "frames," "hitches," "desyncs," "builds"
- Let the Perfetto screenshot sink in - familiar tool, remote data

---

## Screenshot Capture Guide

Capture from the analytics web app running with Micromegas service telemetry (dogfooding).

### Before capturing:
- [ ] Use dark theme if available (better for presentations)
- [ ] Browser at consistent width (1920px recommended)
- [ ] Hide browser chrome/bookmarks bar

### Screenshots needed:

| Screenshot | Page | What to show |
|------------|------|--------------|
| Processes List | `/processes` | List of processes with exe names, computers, timestamps. Include search bar. |
| Process Overview | `/process` | 5-panel info card: process info, environment, timing, statistics (log/metrics/trace counts), properties |
| Log Viewer | `/process_log` | Log entries table with search filter active, SQL query visible in right panel |
| Metrics Chart | `/process_metrics` | Time series chart with property timeline below, P99/Max toggle, metric selector |
| Performance Analysis | `/performance_analysis` | Metrics chart, thread coverage timeline, "Open in Perfetto" button |
| Perfetto Trace | ui.perfetto.dev | Flame graph with multiple threads, span names from code |
| SQL Query Editor | Any page, right panel | SQL with JOINs, variable values, Execute button |
| Grafana Dashboard | Grafana | Multiple panels, mix of metrics and logs, time range selector |
