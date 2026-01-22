/**
 * Screen renderers initialization.
 * Import this file to register all built-in renderers.
 *
 * Re-exports everything from index.ts for convenience.
 */

// First, export everything from the registry
export * from './index'

// Then import renderers to trigger registration
// Order matters: index.ts must be fully initialized before these run
import './ProcessListRenderer'
import './MetricsRenderer'
import './LogRenderer'
import './TableRenderer'
