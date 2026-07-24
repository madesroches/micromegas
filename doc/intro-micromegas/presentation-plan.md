# Speaker Guide: An Introduction to Micromegas

30-minute talk for a mixed audience (game devs, data scientists, data engineers, managers) that has never used Micromegas. Substance over pitch.

## Key Phrases to Land

Weave these in naturally — these are the lines the audience should be able to repeat after the talk:

- **"Capture enough to fix without reproducing"** — the framing
- **"Simpler AND more powerful"** — the unified-database thesis
- **"One query, complete context"** — the unified payoff in five words
- **"Events should be cheap enough to never sample by default"** — the cost-model inversion
- **"Same goals, very different cost model"** — the biggest-difference framing
- **"900 billion events for $1,750 a month"** — the proof. Say the number, then pause.
- **"Memcpy into a thread-local buffer"** — the mechanism behind the 20 ns claim
- **"We didn't reinvent the analytics stack — we assembled it"** — the standards pitch
- **"Same engine, end to end"** — DataFusion server + DataFusion in browser
- **"Move queries, not data"** — the federation pitch

## Don'ts

- **Don't name-and-compare specific vendors.** The "biggest difference" beat is framed as a contrast with the *shared assumption* of modern observability stacks — not as Micromegas-vs-OTel or Micromegas-vs-Datadog. Naming a competitor on stage invites a feature-by-feature derail.
- Don't dive into LZ4 / Arrow IPC encoding mechanics — the audience cares about what it enables.
- Don't run a live demo. This talk is screenshot-only by design (see trade-offs in the design plan).
- Don't read the slides. Fragment lists are cues, not a script.
- Don't tune the talk for one cohort — every section should land for at least one of the four audiences.

## Do's

- Use the audience's vocabulary (services, requests, spans) and bridge to Micromegas's (events, streams, blocks).
- **Pause on the 20 ns number** — let it land.
- **Pause on the cost number** — "$1,750 a month for 900 billion events" is the line the audience will quote to their boss. Don't bury it in a list.
- **Pause on "one query, complete context"** — let the audience think of their last cross-tool investigation.
- When asked "how does this compare to <X>?" in Q&A, answer it then. On stage, stay focused on what Micromegas does.

## Audience Anchors (which section speaks to whom)

- **Game devs** → §6 (instrumentation overhead, "what this unlocks")
- **Data scientists** → §7 (SQL), §8 (Python API + `pip install micromegas`), §9 (notebooks)
- **Data engineers** → §5 (architecture, data lake / lakehouse split), §8 (HTTP gateway + Databricks federation)
- **Managers** → §2 ("fix without reproducing"), §4 (simpler-and-more-powerful), §5 cost slide

## Time Budget

| Section | Slides | Time | Cumulative |
|---|---|---|---|
| 1. Title + framing | 1 | 1:00 | 1:00 |
| 2. The problem | 3 | 3:30 | 4:30 |
| 3. What Micromegas is | 1 | 1:00 | 5:30 |
| 4. Why unified | 2 | 3:00 | 8:30 |
| 5. Architecture + cost | 2 | 3:30 | 12:00 |
| 6. Instrumentation | 4 | 4:00 | 16:00 |
| 7. SQL on Arrow/FlightSQL | 3 | 3:30 | 19:30 |
| 8. Interoperability | 2 | 3:00 | 22:30 |
| 9. Notebooks | 2 | 3:00 | 25:30 |
| 10. Closing | 2 | 2:00 | 27:30 |
| Q&A buffer | — | 2:30 | 30:00 |

If running long, cut: §9's second screenshot, the closer fragment of §4, the implicit-comparison line on the cost slide. **Do not** cut from the unified-database thesis (§4), the cost-efficiency slide (§5), the biggest-difference beat (§6), or interoperability (§8).

## Pre-Stage Checklist (no live demo — screenshots only)

- [ ] All screenshots embedded in `presentation.md` (verify they render in `yarn dev`)
- [ ] Standalone HTML build verified (`yarn build:standalone`) — opens cleanly from `file://`
- [ ] Backup copy of standalone HTML on USB / cloud drive
- [ ] Run through the full deck on the actual presentation laptop once
- [ ] Confirm timer fits inside 28 minutes — leaves real Q&A space

## Objection Handling

| Objection | Response |
|---|---|
| "How does this compare to OTel / Datadog / Honeycomb / Prometheus?" | Same goals, different cost model. The others assume events are individually expensive (sampling mandatory at scale). Micromegas assumes events are cheap and always-on. Different starting assumption, different design. Can run alongside. |
| "Why SQL and not PromQL?" | SQL covers PromQL's analytical use cases plus joins across signals. PromQL specializes; SQL generalizes. |
| "Is FlightSQL mature?" | Yes — Arrow ecosystem standard. Clients in Python, Go (Grafana plugin), Rust, Java. |
| "Can I query from non-gRPC environments?" | Yes — HTTP gateway translates JSON-over-HTTP to FlightSQL. Curl works. |
| "Can I join with my warehouse data?" | Yes via Databricks Lakehouse Federation through the gateway — predicates push down, only results transfer. Caveat: schema isn't auto-discovered today, you declare the tables you want to expose. |
| "What's the migration cost?" | Frame as complement-then-replace, not rip-and-replace. Run alongside existing stack; move signals over as the team gets comfortable. |
| "Do I have to self-host?" | Today, yes — that's also why the cost is what it is. You own the data and the bill. |
| "Can it handle <my workload>?" | Production reference: 900B events / 90 days / ~$1,750/month, spikes of 266M events/min. If your workload is in that ballpark, yes. |
