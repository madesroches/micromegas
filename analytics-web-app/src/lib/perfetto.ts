/**
 * Perfetto integration utilities for opening traces in ui.perfetto.dev
 */

export interface OpenPerfettoOptions {
  buffer: ArrayBuffer
  processId: string
  timeRange: { begin: string; end: string }
}

export interface PerfettoError {
  type: 'popup_blocked' | 'timeout' | 'unknown'
  message: string
}

const PERFETTO_HANDSHAKE_TIMEOUT_MS = 10000
const PERFETTO_PING_INTERVAL_MS = 50

/**
 * Opens a Perfetto trace in ui.perfetto.dev
 *
 * Based on the technique from rust/public/src/servers/perfetto/show_trace.html:
 * 1. Calculate time range in nanoseconds for Perfetto URL params
 * 2. Open Perfetto UI with visStart/visEnd params
 * 3. Ping/pong handshake with postMessage
 * 4. Send trace buffer via postMessage on PONG response
 */
export async function openInPerfetto(options: OpenPerfettoOptions): Promise<void> {
  const { buffer, processId, timeRange } = options

  // Calculate time range in nanoseconds for Perfetto URL
  const beginMs = Date.parse(timeRange.begin)
  const beginNs = beginMs * 1_000_000
  const endMs = Date.parse(timeRange.end)
  const endNs = endMs * 1_000_000

  const perfettoUrl = `https://ui.perfetto.dev/#!/?visStart=${beginNs}&visEnd=${endNs}`

  // Open Perfetto UI
  const win = window.open(perfettoUrl)
  if (!win) {
    const error: PerfettoError = {
      type: 'popup_blocked',
      message: 'Popup blocked. Please allow popups for this site to open Perfetto.',
    }
    throw error
  }

  return new Promise((resolve, reject) => {
    // Set up timeout for handshake
    const timeoutId = setTimeout(() => {
      window.removeEventListener('message', onMessageHandler)
      clearInterval(pingTimer)
      const error: PerfettoError = {
        type: 'timeout',
        message: 'Could not connect to Perfetto UI. Try again or download the trace.',
      }
      reject(error)
    }, PERFETTO_HANDSHAKE_TIMEOUT_MS)

    // Ping Perfetto until we get a PONG
    const pingTimer = setInterval(() => {
      try {
        win.postMessage('PING', perfettoUrl)
      } catch {
        // Window might be closed, ignore
      }
    }, PERFETTO_PING_INTERVAL_MS)

    const onMessageHandler = (evt: MessageEvent) => {
      if (evt.data !== 'PONG') return

      // We got a PONG, the UI is ready
      clearTimeout(timeoutId)
      clearInterval(pingTimer)
      window.removeEventListener('message', onMessageHandler)

      // Send the trace buffer to Perfetto
      try {
        win.postMessage(
          {
            perfetto: {
              buffer: buffer,
              title: `Micromegas trace of process ${processId}`,
            },
          },
          perfettoUrl
        )
        resolve()
      } catch (err) {
        const error: PerfettoError = {
          type: 'unknown',
          message: err instanceof Error ? err.message : 'Failed to send trace to Perfetto',
        }
        reject(error)
      }
    }

    window.addEventListener('message', onMessageHandler)
  })
}
