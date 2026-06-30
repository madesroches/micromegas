---
date: 2026-06-30
authors:
  - madesroches
categories:
  - Engineering
tags:
  - observability
  - blender
  - telemetry
  - crash-reporting
  - ai
  - candor
  - open-source
---

# Stop Guessing Why Blender Crashed

An artist files a ticket: "Blender crashed while I was sculpting." No .blend, no repro steps, no idea which add-ons were running. You try to reproduce it, fail, and close it *cannot reproduce*. Every team that supports a DCC tool knows this loop.

<!-- more -->

## A crash signature isn't a root cause

A stack trace tells you *where* it died, not *why*. To actually fix it you need the things you can't recover after the fact: what the artist was doing, what hardware and add-ons they were running, and what the session looked like on the way down.

Reproducing the bug is the usual way to get that context back. The belief running through everything we build at Micromegas is that you shouldn't have to. Capture enough, correlate it, and the report *is* the repro.

## What the add-on captures — and why

The Micromegas Blender add-on records the session and ships it off the machine. Every signal earns its place by answering a question you'd otherwise have to reproduce the crash to answer:

- **The crash itself** — Blender's own crash report, harvested on the next launch and tied to the session that died.
- **What the artist did** — every operator, mode switch, and undo/redo, in order. Reconstruct the exact sequence into the crash instead of asking "so, what were you doing?"
- **What they were running** — GPU vendor, renderer, and **driver**; the exact Blender build; and the **enabled third-party add-ons with versions**. GPU and driver are the number-one crash dimension for a DCC tool, and a bad add-on is the leading cause of Blender instability. These are the things you slice by to find the pattern.
- **The shape of the session** — resident memory, render durations, viewport update latency, scene complexity, sampled throughout. So you can watch memory climb or performance degrade *before* the failure, not just see the wreckage.
- **The exceptions** — unhandled Python errors, shipped with full tracebacks.

Every event carries the same session fingerprint, so all of it lines up against a single failure.

## One place, not three systems

Because crashes, actions, metrics, and process lifecycles land in the same store, questions that used to mean stitching a crash reporter to a metrics tool to a logging service become answerable at all. "Did this add-on version correlate with the crashes?" stops being three exports and a spreadsheet.

It doesn't slow Blender down, either. The add-on is thin Python; the transport is a Rust library running on its own background thread, so uploads never block the UI. And it's cheap to run across a whole studio — on the order of single-digit megabytes per artist per day.

## Blender already records most of this — on the artist's machine

None of these signals are exotic. Blender writes a crash log on its way down, prints Python tracebacks to the system console, and the Info editor already shows your recent operator history. The catch is where it all lives: on the artist's workstation, for the session in front of them, uncorrelated, and gone the moment they close the file or clear the editor. To use any of it you have to reach the right machine and ask the right person to dig it out — which, for a crash you can't reproduce, is exactly the part that fails.

The add-on doesn't invent new telemetry so much as move what's already there off the machine, stamp it with a session, and keep it — so the next time a crash report comes in, the context is already waiting instead of lost.

## You just ask

Here's the real payoff, and it's why there's not a line of query syntax in this post. Because everything lands in one place reachable from a plain command-line tool, a foundation model does the last mile — so *you* never write a query.

Point Claude at it and ask in plain English: *"Which sessions crashed on the latest NVIDIA driver this week, and what were the artists doing right before?"* It pulls the crashes, the actions leading in, and the hardware fingerprint, tells you what it found — and when you ask a follow-up, it digs further. No MCP server, no RAG pipeline, no training: the entire integration is a markdown file describing the data. The conversation *is* the investigation. (More on that in [From Observability to Candor](2026-03-29-from-o11y-to-candor.md).)

Root-cause analysis without reproduction stops being a workflow you run and becomes a question you ask.

## One more thing

The add-on is a thin wrapper over a tiny C interface — about six functions. Blender happens to call it from Python, but anything that can call C can now feed Micromegas. Python is the first proof; C++, C#, Go, and Lua are all straightforward from here. If you want to instrument something, the door's open.

---

The Blender add-on needs Blender 4.2+ (x86-64, Linux or Windows) and a reachable Micromegas ingestion server. See the [Blender docs](../../blender/index.md) to get started, or the [project on GitHub](https://github.com/madesroches/micromegas).
