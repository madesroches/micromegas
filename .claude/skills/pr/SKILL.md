---
name: pr
description: Run lints/tests, finalize plan file, and create a pull request
argument-hint: ""
allowed-tools: Bash(git *), Bash(cargo *), Bash(yarn *), Bash(poetry *), Bash(gh *), Bash(python3 *), Bash(mv *), Bash(mkdir *), Bash(ls *), Read, Glob, Grep, Edit, Write, Task, AskUserQuestion
---

# Pull Request — Lint, Test, and Submit

Create a pull request for the current branch after running lints/tests and finalizing any associated plan file.

## Process

### Phase 1: Gather Context

1. `git branch --show-current` — identify the current branch
2. `git log --oneline main..HEAD` — list commits in this branch
3. `git diff main..HEAD --stat` — file summary to determine which areas changed (Rust, Python, JS/TS, Grafana)
4. Identify the originating GitHub issue:
   - First, check the plan file found in `tasks/` (see Phase 3) — plans typically have `**GitHub Issue**: #123` or a URL on line 3
   - If no plan file, check commit messages and branch name for issue references (e.g., `#123`, `issue-123`)
   - If not found there, search GitHub issues: `gh issue list --search "<branch name or feature keywords>" --state open` and `--state closed`
   - If still no match, ask the user with AskUserQuestion

### Phase 2: Run Lints and Tests

Based on which files changed, run the relevant checks. Launch independent checks in parallel using the Task tool with `subagent_type: "Bash"`.

**Rust** (if `rust/` files changed):
```
cd rust && cargo fmt --check
cd rust && cargo clippy --workspace -- -D warnings
cd rust && cargo test
```

**Python** (if `python/` files changed):
```
cd python/micromegas && poetry run black --check .
cd python/micromegas && poetry run pytest
```

**Analytics Web App** (if `analytics-web-app/` files changed):
```
cd analytics-web-app && yarn lint
cd analytics-web-app && yarn type-check
cd analytics-web-app && yarn test
```

**Grafana Plugin** (if `grafana/` files changed):
```
cd grafana && yarn lint:fix
cd grafana && yarn build
```

If any check fails, report the failures and stop. Do NOT create the PR. Tell the user what needs to be fixed.

### Phase 3: Plan File Management

The plan file should already be part of the branch's diff (moved/updated and committed). Verify this:

1. Check `git diff main..HEAD --stat` for plan file changes in `tasks/`
2. If a plan file was moved to `tasks/completed/`, good — note it for the PR description
3. If a plan file exists in `tasks/` (not `completed/`) and relates to this branch:
   - Read it and check if all implementation steps are completed based on the commits
   - If incomplete, ask the user: "The plan still has incomplete steps. Should I update it and commit before creating the PR?"
   - If done, move it to `tasks/completed/`, commit the move, then proceed
4. If no plan file is found, that's fine — proceed without one

### Phase 4: Create the Pull Request

1. Check if the branch has been pushed to the remote:
   - `git rev-parse --abbrev-ref --symbolic-full-name @{u}` to check tracking
   - If not pushed or behind, push with `git push -u origin <branch>`

2. Build the PR description:
   - Title: concise summary of the changes (under 70 characters)
   - Body must include:
     - `## Summary` — bullet points describing what changed and why
     - `Closes #<issue>` or `Fixes #<issue>` to link the originating issue (use `Closes` for features, `Fixes` for bugs)
     - `## Test plan` — how to verify the changes work
     - If a plan file was moved to completed, mention it: "Design plan: `tasks/completed/<file>`"

3. Create the PR:
```
gh pr create --title "<title>" --body "$(cat <<'EOF'
## Summary
<bullets>

<Closes/Fixes #issue>

## Test plan
<verification steps>
EOF
)"
```

4. Output the PR URL when done.

## Guidelines

- Never force-push or amend commits during this process
- If lints fail with auto-fixable issues (e.g., formatting), ask the user if they want you to fix and commit before retrying
- The issue link is important — always try to find it before falling back to asking
- Keep the PR title short and descriptive — put details in the body
- Match the existing PR style in the repository
