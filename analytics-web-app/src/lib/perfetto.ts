/**
 * Perfetto integration utilities for opening traces in ui.perfetto.dev
 */

export interface OpenPerfettoOptions {
  buffer: ArrayBuffer
  processId: string
  timeRange: { begin: string; end: string }
  onProgress?: (message: string) => void
}

export interface PerfettoError {
  type: 'popup_blocked' | 'timeout' | 'window_closed' | 'unknown'
  message: string
}

const PERFETTO_ORIGIN = 'https://ui.perfetto.dev'
const PERFETTO_HANDSHAKE_TIMEOUT_MS = 20000 // Increased from 10s to 20s
const PERFETTO_PING_INTERVAL_MS = 50
const PERFETTO_WINDOW_CHECK_INTERVAL_MS = 500

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
  const { buffer, processId, timeRange, onProgress } = options

  // Calculate time range in nanoseconds for Perfetto URL
  const beginMs = Date.parse(timeRange.begin)
  const beginNs = beginMs * 1_000_000
  const endMs = Date.parse(timeRange.end)
  const endNs = endMs * 1_000_000

  const perfettoUrl = `${PERFETTO_ORIGIN}/#!/?visStart=${beginNs}&visEnd=${endNs}`

  onProgress?.('Opening Perfetto UI...')

  // Open Perfetto UI
  const win = window.open(perfettoUrl)
  if (!win) {
    const error: PerfettoError = {
      type: 'popup_blocked',
      message: 'Popup blocked. Please allow popups for this site to open Perfetto.',
    }
    throw error
  }

  onProgress?.('Waiting for Perfetto UI to initialize...')

  return new Promise((resolve, reject) => {
    let pingCount = 0

    // Check if user closed the Perfetto window
    const windowCheckTimer = setInterval(() => {
      if (win.closed) {
        cleanup()
        const error: PerfettoError = {
          type: 'window_closed',
          message: 'Perfetto window was closed. Click "Open in Perfetto" to try again.',
        }
        reject(error)
      }
    }, PERFETTO_WINDOW_CHECK_INTERVAL_MS)

    // Set up timeout for handshake
    const timeoutId = setTimeout(() => {
      cleanup()
      const error: PerfettoError = {
        type: 'timeout',
        message:
          'Could not connect to Perfetto UI after 20 seconds. The Perfetto site may be slow or unavailable. You can download the trace instead.',
      }
      reject(error)
    }, PERFETTO_HANDSHAKE_TIMEOUT_MS)

    // Ping Perfetto until we get a PONG
    const pingTimer = setInterval(() => {
      try {
        win.postMessage('PING', PERFETTO_ORIGIN)
        pingCount++
        // Update progress every 2 seconds
        if (pingCount % 40 === 0) {
          const seconds = Math.floor((pingCount * PERFETTO_PING_INTERVAL_MS) / 1000)
          onProgress?.(`Waiting for Perfetto... (${seconds}s)`)
        }
      } catch {
        // Window might be closed, ignore - windowCheckTimer will handle it
      }
    }, PERFETTO_PING_INTERVAL_MS)

    const cleanup = () => {
      clearTimeout(timeoutId)
      clearInterval(pingTimer)
      clearInterval(windowCheckTimer)
      window.removeEventListener('message', onMessageHandler)
    }

    const onMessageHandler = (evt: MessageEvent) => {
      // Verify the message is from Perfetto
      if (evt.origin !== PERFETTO_ORIGIN) return
      if (evt.data !== 'PONG') return

      // We got a PONG, the UI is ready
      cleanup()

      onProgress?.('Sending trace to Perfetto...')

      // Send the trace buffer to Perfetto
      try {
        win.postMessage(
          {
            perfetto: {
              buffer: buffer,
              title: `Micromegas trace of process ${processId}`,
            },
          },
          PERFETTO_ORIGIN
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
