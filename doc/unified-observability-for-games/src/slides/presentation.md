<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

## Unified Observability for Games

<p style="font-size: 0.6em;">Marc-Antoine Desroches · <a href="mailto:madesroches@gmail.com">madesroches@gmail.com</a><br><a href="https://github.com/madesroches/micromegas">github.com/madesroches/micromegas</a></p>

---

## The Pain

The debugging loop everyone hates:

<ol style="font-size: 0.8em;">
<li class="fragment">Receive bug report: "Game hitched during combat"</li>
<li class="fragment">Try to reproduce → fail</li>
<li class="fragment">Ask for more details → wait</li>
<li class="fragment">Try again → can't replicate exact conditions</li>
<li class="fragment">Mark as "cannot reproduce" or waste days guessing</li>
</ol>

--

## The Hard Truth

<ul>
<li class="fragment">Some bugs <strong>can't</strong> be reproduced: race conditions, specific hardware, network timing</li>
<li class="fragment">You only know about <strong>reported</strong> issues - how many players quit silently?</li>
<li class="fragment">Ignored issues kill trust and productivity</li>
</ul>

<p class="fragment" style="margin-top: 2em; color: var(--color-secondary);">What if you could understand issues without reproducing them?</p>

---

## The Goal: Reproduce Less, Fix More

**Stop reproducing bugs. Collect enough data to understand them directly.**

<ul>
<li class="fragment">Quantify <strong>how often</strong> issues happen across your entire player base</li>
<li class="fragment">Quantify <strong>how bad</strong> they are: severity, duration, impact</li>
<li class="fragment">Have <strong>enough context</strong> to fix them without guessing</li>
</ul>

--

## What You Need

<ul>
<li class="fragment"><strong>Logs</strong> - what happened</li>
<li class="fragment"><strong>Metrics</strong> - how bad</li>
<li class="fragment"><strong>Traces</strong> - C++ function calls, asset names</li>
<li class="fragment">All <strong>correlated</strong>, all <strong>queryable</strong></li>
</ul>

--

## A Unified Approach

A unified observability stack makes this practical:

<ul>
<li class="fragment"><strong>Easier</strong>: One system to learn, one query language, one place to look</li>
<li class="fragment"><strong>More powerful</strong>: Automatic correlation unlocks insights fragmented tools can't provide</li>
</ul>

---

## The Case for Unification: Easier

**One system beats three.**

--

## One Query Language

<ul>
<li class="fragment"><strong>SQL</strong>. Everyone knows it (or can learn it - AI can help).</li>
<li class="fragment">No PromQL, no custom DSLs. No learning curve per system.</li>
<li class="fragment">Standard tooling works: Grafana, Python notebooks, your existing BI tools.</li>
</ul>

--

## One Data Model

<ul>
<li class="fragment">Logs, metrics, traces, player events - all timestamped events</li>
<li class="fragment">Same schema concepts across data types</li>
</ul>

--

## One Place to Look

<ul>
<li class="fragment">Stop asking "which tool has this data?"</li>
<li class="fragment">Stop context-switching between UIs</li>
<li class="fragment">Stop maintaining tribal knowledge of "use tool X for problem Y"</li>
</ul>

--

## One Integration

<ul>
<li class="fragment">One SDK to add to your game</li>
<li class="fragment">One endpoint to configure</li>
<li class="fragment">One data format</li>
<li class="fragment">One telemetry protocol</li>
</ul>

--

## One Infra

<ul>
<li class="fragment">Single retention policy to manage</li>
<li class="fragment">One system to update and monitor</li>
<li class="fragment">One team can own the whole stack</li>
<li class="fragment">Capacity planning in one place</li>
<li class="fragment">Simplified budget tracking</li>
</ul>

---

## The Case for Unification: More Powerful

**Automatic correlation unlocks questions you couldn't ask before.**

Every event shares: process, thread, session, player, map, build, precise timestamps.

--

## One Query, Complete Context

<ul style="font-size: 0.8em;">
<li class="fragment">"Show me the CPU trace, logs, and metrics from the frame where this player hitched"</li>
<li class="fragment">"What was the server doing when this client reported a desync?"</li>
<li class="fragment">"All events from this player's session, 10 seconds before the crash"</li>
</ul>

<p class="fragment" style="margin-top: 1em;"><strong>Fragmented:</strong> hours hunting through different tools. <BR><strong>Unified:</strong> one query.</p>

--

## Player Events Too

<ul>
<li class="fragment">Same pipeline as logs, metrics, and traces</li>
<li class="fragment">Unlimited frequency - capture every action, no sampling</li>
<li class="fragment">"Players who crashed - what were they doing right before?"</li>
</ul>

---

## Interoperability: Unified, Not Isolated

**Standard protocols make integration easy.**

--

## Export What You Need

<ul>
<li class="fragment">Python API for programmatic access</li>
<li class="fragment">Tail-sample high-frequency data, export the subset you care about</li>
<li class="fragment">Feed into ML pipelines, external dashboards, long-term archives</li>
</ul>

--

## Standard Protocols

<ul>
<li class="fragment">Analytics service speaks <strong>FlightSQL</strong> - an open standard</li>
<li class="fragment">Existing FlightSQL clients: Python, Grafana (Go), Rust</li>
<li class="fragment">No proprietary lock-in</li>
</ul>

--

## Query Federation

<ul>
<li class="fragment">Plug other systems into the same query layer</li>
<li class="fragment">Join your unified data with external sources</li>
<li class="fragment">HTTP gateway service for easy integration from any language</li>
</ul>

---

## Grafana Dashboard

<div style="display: flex; align-items: center; gap: 2rem;">
<div style="flex: 1;">

Standard tooling, familiar interface.

</div>
<div style="flex: 1;">
<img src="./grafana_monitoring.png" style="max-height: 500px; width: auto;">
</div>
</div>

---

## Perfetto Trace Viewer

<img src="./perfetto_screenshot.png" style="max-width: 100%; height: auto;">

Detailed CPU trace analysis - familiar tool, remote data

---

## Getting Started

**Start alongside your existing tools. Prove value. Expand.**

--

## Week by Week

<ul style="font-size: 0.85em;">
<li class="fragment"><strong>Week 1</strong>: Stand up infrastructure (Terraform templates available)</li>
<li class="fragment"><strong>Week 2</strong>: Install Unreal plugin
  <ul>
    <li>UE_LOG, metrics, and spikes auto-captured</li>
    <li>Data starts flowing immediately</li>
    <li>See data in Grafana and analytics web app</li>
  </ul>
</li>
<li class="fragment"><strong>Week 3</strong>: Teach teams to query and build dashboards</li>
<li class="fragment"><strong>Ongoing</strong>: Expand instrumentation based on what you learn</li>
</ul>

--

## No Rip-and-Replace

<p class="fragment">Run Micromegas alongside your existing tools.</p>
<p class="fragment">When it proves value, migrate.</p>

---

## In Conclusion

Unified observability is **easier** (one system, one language, one place)

AND **more powerful** (automatic correlation, queries you couldn't run before).

--

<div style="font-size: 0.75em;">

| Fragmented | Unified |
|------------|---------|
| Multiple tools to learn | One query language |
| Manual correlation | Automatic correlation |
| Context in your head | Context in the data |
| Each system maintained separately | One system to improve |
| "Cannot reproduce" | "Here's what happened" |
| "Where is the log?" | "I can see others are having this issue" |

</div>

---

<!-- .slide: data-state="hide-sidebar" -->
<img src="./micromegas-vertical-compact.svg" alt="micromegas" class="plain" style="height: 340px; margin: 0;">

**https://github.com/madesroches/micromegas**

Happy to help with initial setup and infra management

