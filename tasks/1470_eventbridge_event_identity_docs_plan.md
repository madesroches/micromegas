# EventBridge `input_transformer` Event-Identity Attribute Convention (Docs) Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1470

## Overview

Document a recommended attribute convention for carrying a source event's identity
(`$.id`) and time (`$.time`) through the AWS EventBridge `input_transformer` → OTLP/JSON
API Destination ingestion path, analogous to the existing `aws.log.event.id` convention
documented for the CloudWatch Logs → Kinesis Firehose path. This is a documentation-only
change: no code, no schema, no new attribute is enforced server-side — the point is to
give producers a name to standardize on instead of inventing one per integration, and to
cross-link the guidance from the incident (#1462) and the design proposal (#1466) that
motivate it.

## Current State

`mkdocs/docs/otlp/index.md` documents two AWS-sourced ingestion paths with different
levels of event-identity guidance:

- **CloudWatch Logs (Kinesis Firehose)** (`## CloudWatch Logs (Kinesis Firehose)`,
  line 525): the "How `logGroup`/`logStream`/`owner` surface" subsection (line 577)
  explicitly documents that the per-event CloudWatch `id` is attached as a record-level
  attribute named `aws.log.event.id` (line 587-589), "letting you correlate a
  `log_entries` row back to the exact CloudWatch event." The event has a real per-event
  timestamp from CloudWatch itself, so there's no analogous time-fallback note needed
  there (line 609).
- **OTLP/JSON & EventBridge API Destinations** (`## OTLP/JSON & EventBridge API
  Destinations`, line 298): shows an `input_transformer` template producing an
  `ExportLogsServiceRequest`, with `timeUnixNano` mapped from `$.time_ns` (line 309) and
  a note that the value must be a quoted string (line 318). There is **no mention of
  `$.id`** anywhere in this section, and no record-level attribute is suggested for it.
  When `input_transformer` can't produce `timeUnixNano` for arbitrary EventBridge event
  shapes, the fallback isn't the two-level `time_unix_nano` → `observed_time_unix_nano`
  rule in the `## Schema mapping` table (line 136) in isolation — the shown template never
  sets `observedTimeUnixNano` either, so in practice both stay 0. The true chain is
  three-level: `time_unix_nano` → `observed_time_unix_nano` → (if both are still zero) the
  block's ingestion arrival time (`Utc::now()` at block-split time), which is the same
  arrival-time fallback the `## Webhook ingestion` section already documents (line
  344-347) for a different producer path. None of this is cross-referenced from the
  EventBridge section itself.

A producer following only the current EventBridge section has no documented place to put
the source event's `$.id`. #1462 records exactly this gap causing real data loss: a
producer forwarding periodic EventBridge events with no identity attribute and a
constant/null-projected payload collided on the content-hash `block_id`
(`uuid_v5(NS_OTEL_BLOCK_V1, payload_bytes)`, documented in `## Idempotency`, line 222) and
had 72 distinct events silently discarded as duplicates. #1466 is the open design proposal
for dedup on a declared idempotency key (rather than a content hash); its leading open
question is "where does the key live?", noting "a distinguished OTLP attribute is the
obvious answer, since it survives EventBridge-style input transformers."

## Design

Add an `aws.event.id` (and, for completeness, `aws.event.time`) record-level attribute
convention to the EventBridge section, matching the existing `aws.log.*` naming pattern
(`aws.log.event.id`, `aws.log.group.name`, `aws.log.stream.name`) minus the `log.`
segment, since EventBridge events aren't log records. Concretely:

- Extend the `input_transformer` example template (line 302-316) to include a
  `logRecords[].attributes` entry carrying `$.id` as `aws.event.id`.
- Add prose (mirroring the CloudWatch Logs section's "How `logGroup`/`logStream`/`owner`
  surface" wording at line 587-589) stating that forwarding `$.id` this way lets a
  `log_entries` row be correlated back to the exact EventBridge event, and that it is
  queryable via `properties` like any other OTel attribute.
- State the true three-level fallback that applies whenever an `input_transformer`
  can't produce nanosecond time for the event's native timestamp shape:
  `timeUnixNano` (from `$.time_ns`, if the producer's template sets it) →
  `observedTimeUnixNano` (if the producer explicitly sets it — the shown example template
  doesn't) → the block's ingestion arrival time (`Utc::now()` at block-split time) if both
  are absent/zero. That last step is the same arrival-time mechanism already documented in
  the `## Webhook ingestion` section (line 344-347); reference it directly rather than
  restating it as a standalone rule. Optionally document `aws.event.time` as a companion
  attribute for producers who want the original `$.time` preserved verbatim (as a string)
  even when it isn't converted to `timeUnixNano` — useful since EventBridge's `$.time` is
  second-resolution ISO-8601, lossy to reconstruct from a nanosecond fallback alone, and
  since without it a record can silently take on the server's arrival time as its stored
  `time` rather than the event's actual occurrence time.
- Add a note in the `## Idempotency` section (line 222-224) or directly in the
  EventBridge section cross-linking to #1462 (the incident) and #1466 (the open design
  proposal), explaining that a producer with no declared identity in its payload can
  collide on the content-hash `block_id`, and that declaring `aws.event.id` is exactly
  the kind of producer-declared key #1466 is asking for. Phrase this as forward-looking
  ("once #1466 lands, this attribute is a concrete example of...") rather than claiming
  #1466 already changes dedup behavior, since it's still an open proposal.

No changes to `## Attribute encoding`, `## Schema mapping`, or any Rust source are needed
— this only adds documented convention for an attribute name the ingestion pipeline
already passes through generically as any other OTel `LogRecord` attribute.

## Implementation Steps

1. In `mkdocs/docs/otlp/index.md`, extend the `input_transformer` JSON template in the
   `## OTLP/JSON & EventBridge API Destinations` section (line 302-316) to add an
   `"attributes"` array to the `logRecords[0]` entry with `aws.event.id` mapped from
   `<$.id>`.
2. Add a short paragraph after the template (after line 318) documenting:
   - `aws.event.id` as the recommended record-level attribute for the source event's
     `$.id`, analogous to `aws.log.event.id` for CloudWatch Logs — link to the
     [CloudWatch Logs](#cloudwatch-logs-kinesis-firehose) section's equivalent guidance.
   - The true three-level timestamp fallback, stated directly: `timeUnixNano` (from
     `$.time_ns`, if set) → `observedTimeUnixNano` (if the producer explicitly sets it —
     the shown template doesn't) → the block's ingestion arrival time (`Utc::now()` at
     block-split time) if both are absent/zero. That last step is the identical
     arrival-time mechanism the `## Webhook ingestion` section already documents (line
     344-347) — link to it rather than restate it, instead of (incorrectly) treating it as
     unrelated. Optionally also link to `#schema-mapping` for the two-level
     `time_unix_nano`/`observed_time_unix_nano` → `time` column mapping.
   - Optionally, `aws.event.time` as a companion attribute for preserving the original
     `$.time` string when timestamp conversion isn't possible.
3. Add a short note (in the EventBridge section and/or the `## Idempotency` section)
   cross-linking #1462 and #1466: declaring `aws.event.id` avoids the content-hash
   collision failure mode from #1462, and is the kind of producer-declared idempotency
   key #1466 proposes formalizing.
4. Re-read the full `## OTLP/JSON & EventBridge API Destinations` section end-to-end to
   confirm it reads coherently with the addition and doesn't duplicate the Webhook
   section's fallback note verbatim in a confusing way.
5. Update `CHANGELOG.md` per the `pr` skill's normal process (handled downstream, not a
   manual step here).

## Files to Modify

- `mkdocs/docs/otlp/index.md` — the only file touched.

## Trade-offs

- **`aws.event.id` vs. reusing `aws.log.event.id`.** Reusing the CloudWatch name would
  make queries across both paths uniform, but `aws.log.event.id` is namespaced under
  `aws.log.*` alongside `aws.log.group.name`/`aws.log.stream.name`, which are CloudWatch
  Logs-specific concepts that don't exist for EventBridge. A new `aws.event.*` namespace
  (mirroring but not colliding with `aws.log.*`) is clearer about what produced the
  event and follows the issue's own suggested name. Chosen: `aws.event.id` /
  `aws.event.time`.
- **Whether to document `aws.event.time` at all.** The full fallback chain
  (`timeUnixNano` → `observedTimeUnixNano` → block ingestion arrival time) is already
  documented (partly in `## Schema mapping`, partly in `## Webhook ingestion`), so
  `aws.event.time` isn't strictly required for the pipeline to keep working. But the real
  risk of omitting `$.time`/`timeUnixNano` isn't just losing sub-second precision — when
  both `timeUnixNano` and `observedTimeUnixNano` are absent, the stored `log_entries.time`
  silently becomes an unrelated server-arrival timestamp (block-split wall-clock time)
  rather than the event's actual occurrence time, with no indication in the row that this
  happened. Documenting `aws.event.time` as optional gives producers who care about
  preserving the exact original timestamp string a documented place to put it, mitigating
  that risk, at the cost of one more attribute name to explain. Included as a documented
  option, not a requirement, since the issue's proposal explicitly calls out `$.time`
  handling as a gap.
- **Where to put the #1462/#1466 cross-link.** Could go solely in the EventBridge section
  (closest to the new convention) or solely in `## Idempotency` (closest to the
  content-hash mechanism the incident is about). Chosen: a link in the EventBridge
  section pointing to `## Idempotency`, with the #1462/#1466 references added to
  `## Idempotency` itself, so the identity-loss mechanism and its citations live in one
  place and the EventBridge section stays focused on "what attribute to send."

## Documentation

- `mkdocs/docs/otlp/index.md` — `## OTLP/JSON & EventBridge API Destinations` and
  `## Idempotency` sections, as described above. This plan touches no other doc page.

## Testing Strategy

Docs-only change; no automated tests apply. Verify by:
- Running the mkdocs local server (`mkdocs/serve.py`) and visually checking the rendered
  section for correct Markdown/table rendering and that intra-page anchors resolve.
- Confirming the `input_transformer` JSON template is valid JSON (e.g. via a JSON linter
  or manual inspection) and that `<$.id>` follows the same quoting convention already
  established for `<$.time_ns>` (a JSONPath substitution inside a JSON string value).

## Open Questions

- **Exact final attribute name(s).** The issue explicitly leaves `aws.event.id` /
  `aws.event.time` open to maintainer preference for consistency with `aws.log.event.id`.
  This plan adopts `aws.event.id` / `aws.event.time` as the working names; flag during
  review if a different name is preferred before implementation.
