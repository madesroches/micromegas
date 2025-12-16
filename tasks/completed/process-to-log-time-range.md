# Process Info Screen Time Range Improvement

## Problem

Currently, the Processes list screen passes `start_time` as `from` and `last_update_time` as `to` when navigating to the Process Info screen (see `ProcessesPage.tsx:306`). The Process Info screen then passes this same time range to the Log and Metrics screens.

This has two issues:

1. **Live processes**: The `last_update_time` is a snapshot from when the processes query ran. Live processes continue emitting logs after this, so users won't see the most recent data.

2. **Long-running processes** (live or dead): Using `start_time` as the begin could span days or weeks, resulting in slow queries and overwhelming amounts of data when the user likely only cares about recent activity.

## Solution

Compute a smarter time range based on the process's actual activity when navigating from Process Info to Log and Metrics screens.

### End Time Logic

1. Capture the current time when the Process Info screen loads (not when the user clicks a link)
2. Calculate time since last activity: `screenLoadTime - process.last_update_time`
3. If the process has recent data (time since last activity < 2 minutes):
   - Use `"now"` as the end time (relative format for live updates)
4. If the process is dead (time since last activity >= 2 minutes):
   - Use `process.last_update_time` as the end time (absolute ISO timestamp)

Note: Using screen load time ensures a process that was live when viewed doesn't falsely appear dead if the user waits before clicking.

### Begin Time Logic

Currently the inherited time range uses `start_time` as the begin, which could be a very long time ago for long-running processes. We want to limit the query window to one hour for better performance and relevance.

1. Calculate one hour before the end time
2. Compare with `process.start_time`
3. Use whichever is **more recent** (the later of the two):
   - If `start_time > (end_time - 1 hour)`: use `start_time` (short-lived process, show everything)
   - Otherwise: use `end_time - 1 hour` (long-running process, show last hour)

## Implementation

### Files to Modify

1. `analytics-web-app/src/routes/ProcessPage.tsx`

### Changes

In `ProcessPageContent`:

1. Capture screen load time when process data is loaded:

```typescript
const [screenLoadTime] = useState(() => new Date())
```

2. Create a helper function to compute the time range:

```typescript
function computeProcessTimeRange(
  screenLoadTime: Date,
  startTime: string | null,
  lastUpdateTime: string | null
): { from: string; to: string } {
  const TWO_MINUTES_MS = 2 * 60 * 1000
  const ONE_HOUR_MS = 60 * 60 * 1000

  // Parse last update time
  const lastUpdate = lastUpdateTime ? new Date(lastUpdateTime) : screenLoadTime
  const timeSinceLastUpdate = screenLoadTime.getTime() - lastUpdate.getTime()

  // Determine end time
  let endTime: Date
  let toValue: string
  if (timeSinceLastUpdate < TWO_MINUTES_MS) {
    // Process is live - use "now" for live updates
    toValue = 'now'
    endTime = screenLoadTime
  } else {
    // Process is dead - use last update time
    endTime = lastUpdate
    toValue = lastUpdate.toISOString()
  }

  // Determine begin time
  const oneHourBeforeEnd = new Date(endTime.getTime() - ONE_HOUR_MS)
  const processStart = startTime ? new Date(startTime) : oneHourBeforeEnd

  // Use the more recent of: process start OR one hour before end
  let fromValue: string
  if (processStart.getTime() > oneHourBeforeEnd.getTime()) {
    fromValue = processStart.toISOString()
  } else {
    fromValue = oneHourBeforeEnd.toISOString()
  }

  return { from: fromValue, to: toValue }
}
```

3. Update the navigation links to use the computed time range:

```typescript
// Before the return statement, compute the time range
const processTimeRange = computeProcessTimeRange(
  screenLoadTime,
  process.start_time as string | null,
  process.last_update_time as string | null
)

// In the AppLink href:
href={`/process_log?process_id=${processId}&from=${encodeURIComponent(processTimeRange.from)}&to=${encodeURIComponent(processTimeRange.to)}`}
```

Apply the same change to View Log, View Metrics, and Performance Analysis links.

### Testing

Test both "View Log" and "View Metrics" buttons for each scenario:

1. **Live process (long-running)**: Navigate to a running process that started > 1 hour ago - should see `from=<1h ago>&to=now` (effectively "last hour")
2. **Live process (short-lived)**: Navigate to a running process that started < 1 hour ago - should see `from=<start_time>&to=now`
3. **Dead process (long-running)**: Navigate to a stopped process that ran > 1 hour - should see `from=<1h before last_update>&to=<last_update_time>`
4. **Dead process (short-lived)**: Navigate to a stopped process that ran < 1 hour - should see `from=<start_time>&to=<last_update_time>`
