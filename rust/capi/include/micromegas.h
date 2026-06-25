/* Micromegas C ABI — telemetry producer for non-Rust processes.
 *
 * Auto-generated layout matches rust/capi/src/lib.rs.
 * Regenerate with cbindgen using rust/capi/cbindgen.toml.
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
 * Emit an integer metric.
 * Keep (name, unit) combinations low-cardinality — each unique pair is
 * permanently interned in memory.
 */
void mm_metric_i(MmHandle *handle, const char *name, const char *unit, uint64_t value);

/**
 * Emit a floating-point metric.
 * Same cardinality contract as mm_metric_i.
 */
void mm_metric_f(MmHandle *handle, const char *name, const char *unit, double value);

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
