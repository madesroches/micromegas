---
name: design-review
description: Review a design plan document with verified issues only
argument-hint: "<path to plan file>"
allowed-tools: Read, Glob, Grep, Bash(git log *), Bash(git diff *), Bash(git show *), Bash(ls *), Bash(clip.exe *), Task
---

# Design Review — Verified Issues Only

Review the design plan at `$ARGUMENTS`.

## Process

### Phase 1: Gather context

1. Read the plan file
2. Read all source files referenced in the plan to understand current state
3. Check `tasks/` and `tasks/completed/` for related plans that establish precedent

### Phase 2: Identify candidate issues

Scan the plan for potential problems:
- **Incorrect assumptions** about existing code (wrong file paths, misunderstood interfaces, stale references)
- **Missing steps** — changes that would be needed but aren't listed (e.g., updating imports, adding exports, migrations)
- **Ordering errors** — steps that depend on something introduced in a later step
- **Contradictions** — the plan says one thing in one section and something different elsewhere
- **Breaking changes** not accounted for — callers, tests, or dependents that would break
- **Over-engineering** — unnecessary abstractions, premature generalization, solving problems that don't exist
- **Under-specification** — critical decisions left vague that will block implementation
- **Pattern violations** — approaches that conflict with established codebase conventions
- **Type safety gaps** — proposed interfaces that lose type information or require unsafe casts
- **Missing error handling** at system boundaries
- **Test strategy gaps** — untestable designs, missing edge cases, no integration coverage
- **Performance concerns** — N+1 queries, unnecessary re-renders, unbounded data structures

For each candidate, write a one-line summary and note which plan section is involved.

### Phase 3: Verify candidates (parallel)

Launch verification agents in parallel using the Task tool with `subagent_type: "Explore"`.

**Grouping strategy:**
- Group candidates that relate to the same area of the codebase into a single agent
- Each agent handles one group of related candidates
- If there are 3 or fewer total candidates, verify them all in a single agent instead of parallelizing

**Each agent prompt must include:**
1. The candidate issue(s) to verify — summary and which plan section is involved
2. The relevant excerpt from the plan for context
3. The verification checklist:
   - Read the actual source files referenced — does the code match what the plan assumes?
   - Are the interfaces, types, and function signatures as the plan describes?
   - Does the proposed change actually conflict with existing code, or does it fit cleanly?
   - Is the "missing step" truly missing, or is it handled implicitly by existing code or tooling?
   - Is the "over-engineering" concern valid, or does the complexity serve a real need evident in the codebase?
   - Are there existing patterns or utilities the plan overlooks that would simplify or invalidate a step?
4. Instructions to return a verdict for each candidate: **confirmed** or **false positive**, with a one-line explanation

Launch all agents in a single message so they run concurrently. Collect all results before proceeding.

### Phase 4: Report

Output a concise list of **confirmed issues only**. For each:
- One-line summary
- Plan section reference
- Why it's a real problem (what you verified against the code)
- Suggested fix (one sentence)

At the end, note how many candidates were dismissed as false positives (no need to list them individually unless the user asks).

### Phase 5: Clipboard

Copy the confirmed issues list to clipboard using `clip.exe`. Keep it terse — one line per issue, no markdown formatting in the clipboard version.
