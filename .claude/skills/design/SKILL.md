---
name: design
description: Research and write a design plan to tasks/ folder
argument-hint: "<feature description, issue link, or context>"
allowed-tools: Read, Glob, Grep, Bash(git log *), Bash(git diff *), Bash(git show *), Bash(ls *), WebFetch, WebSearch, Write, Edit, Task
---

# Design — Research and Write a Plan

Create a design plan for the feature or task described in `$ARGUMENTS` and write it to a markdown file in the `tasks/` folder.

**This skill produces a plan document only. Do NOT write any implementation code.**

## Process

### Phase 1: Understand the Request

Parse `$ARGUMENTS` to identify:
- Feature description or problem statement
- Issue links (fetch with `gh issue view` or WebFetch if provided)
- Any constraints or preferences mentioned

Summarize the goal in one sentence before proceeding.

### Phase 2: Research the Codebase

Explore the codebase to understand the relevant architecture:

1. Identify affected files, modules, and interfaces
2. Understand existing patterns in neighboring code
3. Check for related completed plans in `tasks/` and `tasks/completed/` for precedent
4. Note any existing utilities, types, or abstractions that should be reused
5. Use Task tool with Explore agents for broad searches when needed

### Phase 3: Research External Dependencies (if needed)

If the feature requires new libraries, APIs, or external integrations:
- Search for candidate libraries and evaluate them
- Check compatibility with existing stack
- Note version constraints and bundle size implications

### Phase 4: Write the Plan

Write a markdown file to `tasks/<slug>_plan.md` where `<slug>` is a short snake_case name derived from the feature.

The plan should include these sections (adapt as needed — small tasks need less detail):

```markdown
# <Title> Plan

## Overview
One paragraph: what this does and why.

## Current State
How things work today. Include relevant code paths and file references.

## Design
Technical approach. Include:
- Data structures / type changes
- API or interface changes
- Key algorithms or logic
- Architecture diagrams (ascii) if helpful

## Implementation Steps
Ordered list of concrete steps. Group into phases for larger tasks.
Each step should reference specific files to modify or create.

## Files to Modify
Quick reference list of files that will be touched.

## Trade-offs
What alternatives were considered and why this approach was chosen.

## Testing Strategy
How to verify the implementation works.

## Open Questions
Anything that needs clarification before implementation.
```

Omit sections that don't apply. Add sections that are needed (e.g., Migration, Security, Performance, Dependencies).

### Phase 5: Present the Plan

After writing the file, output:
- The file path
- A brief summary of the approach
- Any open questions that need user input before implementation

## Guidelines

- Reference specific file paths and line numbers when discussing current state
- Keep the plan actionable — someone should be able to implement it from the document alone
- Look at existing plans in `tasks/` and `tasks/completed/` to match the project's level of detail
- Prefer reusing existing patterns over inventing new ones
- Don't over-specify implementation details that are obvious from context
- Keep in mind the open/closed principle
- Keep in mind the DRY principle
- DO NOT write any implementation code — only the plan document
