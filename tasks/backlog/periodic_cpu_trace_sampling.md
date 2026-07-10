# CPU Trace Sampling Gated on User Activity

## Context

Currently, `FSamplingController` sends CPU span data when it detects frame time spikes, regardless of whether the user is actively playing. This wastes bandwidth on idle/AFK sessions. Additionally, normal-operation CPU traces are never captured — only hitches.

Goals:
1. **Gate all CPU trace sampling on user activity** — no spans sent (spike or periodic) unless the user was recently active
2. **Add periodic sampling** — capture a CPU trace snapshot every 2 minutes when the user is active, providing baseline performance data alongside spike-triggered samples

The game's telemetry player-controller component's `UpdateLastInputTime()` already has robust gameplay input detection (movement vectors, rotation input, key presses) covering all controller input including analog sticks.

## Approach

1. Expose a `ReportUserActivity()` method on the `IMicromegasTelemetrySinkModule` public API
2. Have the game's telemetry player-controller component call it when input is detected
3. `SamplingController` uses the last reported activity time to:
   - Skip spike detection entirely when user is idle
   - Trigger periodic sampling every N seconds when user is active

Activity gating is controlled by `telemetry.spans.user_active_threshold_seconds` (default: 0). When 0, the feature is disabled and the system behaves as before. Any positive value enables activity gating: spans are only sampled if `ReportUserActivity()` was called within that many seconds.

## Files to Modify

### 1. `unreal/MicromegasTelemetrySink/Public/MicromegasTelemetrySink/MicromegasTelemetrySinkModule.h`

Add to `IMicromegasTelemetrySinkModule`:
- `virtual void ReportUserActivity() = 0;` — game code calls this when user input is detected

### 2. `unreal/MicromegasTelemetrySink/Private/MicromegasTelemetrySinkModule.cpp`

In `FMicromegasTelemetrySinkModule`:
- Implement `ReportUserActivity()` — forwards to `SamplingController` if available

### 3. `unreal/MicromegasTelemetrySink/Private/SamplingController.h`

Add new members:
- `double LastReportedUserActivityTime` — set by game code via `ReportUserActivity()`, `0.0` = never reported
- `double LastActivityResumeTime` — set when transitioning from idle to active, `0.0` = never resumed
- `TUniquePtr<TAutoConsoleVariable<float>> CVarUserActiveThresholdSeconds` — 0 = disabled, >0 = seconds before user is considered idle (default: 0)
- `TUniquePtr<TAutoConsoleVariable<float>> CVarPeriodicCaptureIntervalSeconds` — configurable periodic interval (default: 0, disabled; set to e.g. 120.0 to enable)

Add new methods:
- `void ReportUserActivity()` — updates `LastReportedUserActivityTime`
- `bool IsUserActive() const` — activity check gated on idle threshold

### 4. `unreal/MicromegasTelemetrySink/Private/SamplingController.cpp`

In the constructor:
- Initialize new members, create CVars

New helper:
- `bool IsPeriodicCaptureDue()`:
  - Read `CVarPeriodicCaptureIntervalSeconds`; return `false` if <= 0
  - Compute reference time: if `SampledTimeRanges` is empty, use `LastActivityResumeTime`; otherwise use `FMath::Max(SampledTimeRanges.Last().Get<1>(), LastActivityResumeTime)`
  - If reference time is `0.0` (no activity ever reported) → return `false`
  - Return `(Now - ReferenceTime) >= IntervalSeconds`

New methods:
- `ReportUserActivity()` — captures `double Now = FPlatformTime::Seconds()` once; if `!IsUserActive()` (transitioning from idle to active), stores `Now` in `LastActivityResumeTime`; then stores `Now` in `LastReportedUserActivityTime`
- `IsUserActive()`:
  - `#if UE_SERVER` → return `true` (no player to report activity; activity gating is meaningless on dedicated servers)
  - If `CVarUserActiveThresholdSeconds <= 0` → return `true` (feature disabled, no gating)
  - Otherwise → return `LastReportedUserActivityTime > 0.0 && (FPlatformTime::Seconds() - LastReportedUserActivityTime) < CVarUserActiveThresholdSeconds`

In `Tick()`:
- Timing bookkeeping (`Now`, `LastFrameDeltaTime`, `FrameTimeRunningAvg.Add()`) runs unconditionally — an early return would leave `LastFrameDateTime` stale, causing the next spike's `TimeRange` to span the entire idle period
- Compute `const bool bUserActive = IsUserActive()`
- Compute `const bool bSpikeDetected = (LastFrameDeltaTime >= RunningAvg * SpikeFactor)`
- Compute `const bool bPeriodicDue = IsPeriodicCaptureDue()`
- Determine if a sample should be taken: `const bool bShouldSample = bSpikeDetected || bPeriodicDue`
- If `bShouldSample && bUserActive` → add `TimeRange(LastFrameDateTime, Now)` to `SampledTimeRanges`, inflate `SpikeFactor` if `bSpikeDetected`, log reason at Verbose (spike/periodic/both)
- If `bShouldSample && !bUserActive` → log suppression at Verbose
- Update `LastFrameDateTime = Now` at the end

Note: periodic capture uses `Max(last sample time, last activity resume time)` as its reference, so both spike triggers and idle→active transitions reset the periodic interval. A spike resets the periodic interval; resuming from idle resets it too (ensuring the first post-idle sample captures steady-state gameplay, not a context-switch frame). Periodic and spike samples look identical from the downstream pipeline's perspective.

### 5. Game-side telemetry player-controller component (consumer project)

In `UpdateLastInputTime()`, after setting `LastUserInputTime`:
- Call `ReportUserActivity()` with a null guard on the module pointer:
  ```cpp
  if (auto* Module = IMicromegasTelemetrySinkModule::GetModulePtr())
  {
      Module->ReportUserActivity();
  }
  ```

## Behavior Matrix

| `user_active_threshold_seconds` | `periodic_capture_interval_seconds` | User active? | Spike detection | Periodic sampling |
|---|---|---|---|---|
| 0 (default) | 0 (default) | N/A | Works as before | Disabled |
| 0 | >0 (e.g. 120) | N/A | Works as before | Active (unconditional) |
| >0 (e.g. 2) | 0 | Active | Active | Disabled |
| >0 (e.g. 2) | 0 | Idle | Suppressed | Disabled |
| >0 (e.g. 2) | >0 (e.g. 120) | Active | Active | Active |
| >0 (e.g. 2) | >0 (e.g. 120) | Idle | Suppressed | Suppressed |

## Design Notes

- No Slate dependency needed — user activity comes from game code via the public API
- The plugin stays game-agnostic: any game using MicromegasTelemetrySink can report activity
- **Orthogonal controls** — activity gating (`user_active_threshold_seconds`) and periodic sampling (`periodic_capture_interval_seconds`) are independent; either can be enabled without the other. Both default to 0 (disabled), preserving existing behavior
- Both `ReportUserActivity()` and `Tick()` run on the game thread, no atomics needed
- With a 2s threshold, sampling stops almost immediately when the user lets go of the controls
- **Dedicated servers**: `IsUserActive()` compiles to `return true` under `UE_SERVER`, so activity gating is stripped out entirely on dedicated server builds. `ReportUserActivity()` is never called (no local player controller), but this is harmless. Periodic sampling defaults to 0 (disabled) on all targets; it can be explicitly enabled per server via CVar if desired, and will fire unconditionally since there is no activity gating
- `CVarSpansAll` still bypasses all sampling logic (sends everything regardless of activity)

## Verification

1. Build the UE project containing the plugin and the consuming game module
2. **Defaults (threshold 0, periodic 0)**: confirm spike detection works exactly as before, no periodic sampling
3. **Periodic only (threshold 0, periodic 120)**: confirm periodic samples fire every 2min unconditionally, spike detection works as before
4. **Spike resets periodic interval**: with periodic 120, trigger a spike at T=60s — confirm the next periodic sample fires at T=180s (120s after the spike), not T=120s
5. **Activity gating only (threshold 2, periodic 0)**:
   - Play actively — confirm spike detection works
   - Go AFK for >2s — confirm spike detection suppressed
   - Resume playing — confirm spike detection resumes
6. **Both (threshold 2, periodic 120)**:
   - Play actively — confirm spike and periodic samples both appear in verbose logs
   - Go AFK for >2s — confirm no new spans are sampled (neither spike nor periodic)
   - Resume playing — confirm sampling resumes; periodic interval resets from resume time (first periodic sample fires a full interval after resume, not immediately)
7. **Periodic and spike are indistinguishable downstream**: confirm `ShouldSampleBlock` treats periodic and spike time ranges identically (both are just entries in `SampledTimeRanges`)
8. Test `telemetry.spans.periodic_capture_interval_seconds 0` disables periodic sampling while spike detection still works
9. Test `telemetry.spans.all 1` bypasses activity check
10. Test various `telemetry.spans.user_active_threshold_seconds` values
11. **Dedicated server**: confirm spike detection works regardless of `user_active_threshold_seconds` value, and periodic sampling only fires when explicitly enabled via CVar
