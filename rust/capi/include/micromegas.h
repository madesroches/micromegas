/* Micromegas C ABI — telemetry producer for non-Rust processes.
 *
 * Maintained by hand, in sync with rust/capi/src/lib.rs (not generated).
 *
 * Threading: all functions are safe to call from any thread.
 * The transport runs on its own OS thread inside the library.
 */

#ifndef MICROMEGAS_H
#define MICROMEGAS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Log level constants — match micromegas_tracing::levels::Level */
#define MM_LEVEL_FATAL  1
#define MM_LEVEL_ERROR  2
#define MM_LEVEL_WARN   3
#define MM_LEVEL_INFO   4
#define MM_LEVEL_DEBUG  5
#define MM_LEVEL_TRACE  6

/**
 * Configuration for mm_init.  All pointer fields may be NULL:
 *   sink_url null    → reads MICROMEGAS_TELEMETRY_URL env var
 *   property arrays  → ignored when property_count == 0
 * Authentication is always read from environment variables:
 *   MICROMEGAS_INGESTION_API_KEY (API key)
 *   MICROMEGAS_OIDC_TOKEN_ENDPOINT / _CLIENT_ID / _CLIENT_SECRET (OIDC)
 */
typedef struct MmConfig {
    const char *sink_url;
    const char **property_keys;
    const char **property_values;
    unsigned int property_count;
} MmConfig;

/** Opaque handle returned by mm_init.  Must not be copied or cast. */
typedef struct MmHandle MmHandle;

/**
 * Initialize the telemetry system.
 * Returns NULL on failure.  The caller owns the returned handle and must
 * eventually pass it to mm_shutdown.
 */
MmHandle *mm_init(const MmConfig *cfg);

/**
 * Flush all pending events and shut down the telemetry system.
 * After this call the handle is invalid.  Safe to call with NULL.
 */
void mm_shutdown(MmHandle *handle);

/**
 * Emit a log event.
 * level: one of the MM_LEVEL_* constants.
 * target: subsystem name, e.g. "blender.render" (NULL → "capi").
 * msg: the log message (NULL is a no-op).
 */
void mm_log(MmHandle *handle, int level, const char *target, const char *msg);

/**
 * Emit a log event with properties.
 * Same as mm_log, plus property_count key/value pairs read from the parallel
 * arrays.  A pair is skipped when its key or value is NULL, or its key is
 * empty or not UTF-8.  "target" and "level" keys are not applied: the
 * arguments decide those.  Keys and values are copied into the current block.
 */
void mm_log_tagged(MmHandle *handle, int level, const char *target, const char *msg,
                   const char *const *property_keys, const char *const *property_values,
                   unsigned int property_count);

/**
 * Emit an integer metric.
 * name must not be NULL or empty; a NULL unit is "".  Both are copied into the
 * current block and freed with it.  Every distinct name is still a distinct
 * series in queries, so keep names drawn from a bounded set.
 */
void mm_metric_i(MmHandle *handle, const char *name, const char *unit, uint64_t value);

/**
 * Emit an integer metric with properties.
 * Properties are read as in mm_log_tagged.  A "target" key replaces the
 * default "capi" target; "name" and "unit" keys are overridden by the arguments.
 */
void mm_metric_i_tagged(MmHandle *handle, const char *name, const char *unit,
                        const char *const *property_keys, const char *const *property_values,
                        unsigned int property_count, uint64_t value);

/**
 * Emit a floating-point metric.
 * Same contract as mm_metric_i.
 */
void mm_metric_f(MmHandle *handle, const char *name, const char *unit, double value);

/**
 * Emit a floating-point metric with properties.
 * Same contract as mm_metric_i_tagged.
 */
void mm_metric_f_tagged(MmHandle *handle, const char *name, const char *unit,
                        const char *const *property_keys, const char *const *property_values,
                        unsigned int property_count, double value);

/**
 * Flush in-memory log and metric buffers.
 * The background transport thread will then upload them.
 * Safe to call with a NULL handle.
 */
void mm_flush(MmHandle *handle);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* MICROMEGAS_H */
