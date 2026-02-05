import { useMemo } from 'react'
import { X } from 'lucide-react'
import type { ScreenConfig } from '@/lib/screens-api'

interface ConfigDiffModalProps {
  isOpen: boolean
  onClose: () => void
  savedConfig: ScreenConfig | null
  currentConfig: ScreenConfig
  currentTimeRange: { from: string; to: string }
}

type DiffStatus = 'unchanged' | 'modified' | 'added' | 'removed'

interface DiffSection {
  title: string
  status: DiffStatus
  lines: DiffLine[]
}

interface DiffLine {
  type: 'context' | 'added' | 'removed'
  content: string
}

interface CellLike {
  name: string
  type: string
  [key: string]: unknown
}

function isCellArray(value: unknown): value is CellLike[] {
  return (
    Array.isArray(value) &&
    value.every(
      (item) =>
        typeof item === 'object' &&
        item !== null &&
        'name' in item &&
        'type' in item
    )
  )
}

function prettyJson(obj: unknown): string {
  return JSON.stringify(obj, null, 2)
}

/**
 * Compute a unified diff between two multi-line strings.
 * Returns diff lines with context/added/removed markers.
 */
function computeLineDiff(oldText: string, newText: string): DiffLine[] {
  const oldLines = oldText.split('\n')
  const newLines = newText.split('\n')
  const lines: DiffLine[] = []

  // Simple LCS-based diff
  const m = oldLines.length
  const n = newLines.length

  // Build LCS table
  const dp: number[][] = Array.from({ length: m + 1 }, () =>
    new Array<number>(n + 1).fill(0)
  )
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      if (oldLines[i - 1] === newLines[j - 1]) {
        dp[i][j] = dp[i - 1][j - 1] + 1
      } else {
        dp[i][j] = Math.max(dp[i - 1][j], dp[i][j - 1])
      }
    }
  }

  // Backtrack to build diff
  const result: DiffLine[] = []
  let i = m
  let j = n
  while (i > 0 || j > 0) {
    if (i > 0 && j > 0 && oldLines[i - 1] === newLines[j - 1]) {
      result.push({ type: 'context', content: oldLines[i - 1] })
      i--
      j--
    } else if (j > 0 && (i === 0 || dp[i][j - 1] >= dp[i - 1][j])) {
      result.push({ type: 'added', content: newLines[j - 1] })
      j--
    } else {
      result.push({ type: 'removed', content: oldLines[i - 1] })
      i--
    }
  }
  result.reverse()

  // Trim leading/trailing context, keep up to 2 lines of context around changes
  const hasChange = result.some((l) => l.type !== 'context')
  if (!hasChange) return lines

  // Find first and last change
  const firstChange = result.findIndex((l) => l.type !== 'context')
  let lastChange = result.length - 1
  while (lastChange >= 0 && result[lastChange].type === 'context') lastChange--

  const contextBefore = Math.max(0, firstChange - 2)
  const contextAfter = Math.min(result.length - 1, lastChange + 2)

  for (let k = contextBefore; k <= contextAfter; k++) {
    lines.push(result[k])
  }

  return lines
}

function computeDiffSections(
  savedConfig: ScreenConfig | null,
  currentConfig: ScreenConfig,
  currentTimeRange: { from: string; to: string }
): DiffSection[] {
  const sections: DiffSection[] = []

  if (!savedConfig) {
    // New screen - everything is "added"
    sections.push({
      title: 'New configuration',
      status: 'added',
      lines: prettyJson(currentConfig)
        .split('\n')
        .map((line) => ({ type: 'added' as const, content: line })),
    })
    return sections
  }

  const savedCells = savedConfig.cells
  const currentCells = currentConfig.cells

  // Handle cell-by-cell diff for notebook-like configs
  if (isCellArray(savedCells) || isCellArray(currentCells)) {
    const saved = isCellArray(savedCells) ? savedCells : []
    const current = isCellArray(currentCells) ? currentCells : []

    // Build maps by name
    const savedMap = new Map(saved.map((c, i) => [c.name, { cell: c, index: i }]))
    const currentMap = new Map(current.map((c, i) => [c.name, { cell: c, index: i }]))

    // Process current cells (preserves order)
    for (const { cell: currentCell, index: idx } of currentMap.values()) {
      const savedEntry = savedMap.get(currentCell.name)
      if (!savedEntry) {
        // Added cell
        sections.push({
          title: `cells[${idx}] \u2014 ${currentCell.name}`,
          status: 'added',
          lines: prettyJson(currentCell)
            .split('\n')
            .map((line) => ({ type: 'added' as const, content: line })),
        })
      } else {
        const savedJson = prettyJson(savedEntry.cell)
        const currentJson = prettyJson(currentCell)
        if (savedJson === currentJson) {
          sections.push({
            title: `cells[${idx}] \u2014 ${currentCell.name}`,
            status: 'unchanged',
            lines: [],
          })
        } else {
          sections.push({
            title: `cells[${idx}] \u2014 ${currentCell.name}`,
            status: 'modified',
            lines: computeLineDiff(savedJson, currentJson),
          })
        }
      }
    }

    // Removed cells (in saved but not in current)
    for (const { cell: savedCell, index: idx } of savedMap.values()) {
      if (!currentMap.has(savedCell.name)) {
        sections.push({
          title: `cells[${idx}] \u2014 ${savedCell.name} (saved)`,
          status: 'removed',
          lines: prettyJson(savedCell)
            .split('\n')
            .map((line) => ({ type: 'removed' as const, content: line })),
        })
      }
    }
  } else {
    // Non-notebook config: diff the whole config (excluding timeRange which is handled below)
    const { timeRangeFrom: _sf, timeRangeTo: _st, ...savedRest } = savedConfig
    const { timeRangeFrom: _cf, timeRangeTo: _ct, ...currentRest } = currentConfig
    const savedJson = prettyJson(savedRest)
    const currentJson = prettyJson(currentRest)
    if (savedJson !== currentJson) {
      sections.push({
        title: 'Configuration',
        status: 'modified',
        lines: computeLineDiff(savedJson, currentJson),
      })
    }
  }

  // Time range diff
  const savedFrom = savedConfig.timeRangeFrom ?? ''
  const savedTo = savedConfig.timeRangeTo ?? ''
  const curFrom = currentTimeRange.from
  const curTo = currentTimeRange.to

  if (savedFrom !== curFrom || savedTo !== curTo) {
    const lines: DiffLine[] = []
    if (savedFrom !== curFrom) {
      lines.push({ type: 'removed', content: `"timeRangeFrom": "${savedFrom}"` })
      lines.push({ type: 'added', content: `"timeRangeFrom": "${curFrom}"` })
    } else {
      lines.push({ type: 'context', content: `"timeRangeFrom": "${curFrom}"` })
    }
    if (savedTo !== curTo) {
      lines.push({ type: 'removed', content: `"timeRangeTo": "${savedTo}"` })
      lines.push({ type: 'added', content: `"timeRangeTo": "${curTo}"` })
    } else {
      lines.push({ type: 'context', content: `"timeRangeTo": "${curTo}"` })
    }
    sections.push({
      title: 'timeRange',
      status: 'modified',
      lines,
    })
  }

  return sections
}

const statusColors: Record<DiffStatus, string> = {
  unchanged: 'var(--text-muted)',
  modified: 'var(--brand-gold)',
  added: 'var(--brand-blue)',
  removed: 'var(--brand-rust)',
}

const statusLabels: Record<DiffStatus, string> = {
  unchanged: 'no changes',
  modified: 'modified',
  added: 'added',
  removed: 'removed',
}

export function ConfigDiffModal({
  isOpen,
  onClose,
  savedConfig,
  currentConfig,
  currentTimeRange,
}: ConfigDiffModalProps) {
  const sections = useMemo(
    () => computeDiffSections(savedConfig, currentConfig, currentTimeRange),
    [savedConfig, currentConfig, currentTimeRange]
  )

  if (!isOpen) return null

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop */}
      <div className="absolute inset-0 bg-black/60" onClick={onClose} />

      {/* Modal */}
      <div className="relative w-full max-w-[700px] max-h-[80vh] bg-app-panel border border-theme-border rounded-lg shadow-xl flex flex-col">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-theme-border flex-shrink-0">
          <div>
            <h2 className="text-lg font-semibold text-theme-text-primary">Configuration Diff</h2>
            <span className="text-xs text-theme-text-muted">Saved vs. Current</span>
          </div>
          <button
            onClick={onClose}
            className="p-1 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="flex-1 overflow-auto p-5">
          {sections.length === 0 && (
            <p className="text-theme-text-muted text-sm">No differences found.</p>
          )}

          {sections.map((section, idx) => (
            <div key={idx} className="mb-4">
              {/* Section title */}
              <div
                className="text-sm font-semibold pb-1.5 mb-2 border-b border-theme-border flex items-center justify-between"
                style={{ color: 'var(--text-secondary)' }}
              >
                <span>{section.title}</span>
                <span
                  className="text-xs font-normal"
                  style={{ color: statusColors[section.status] }}
                >
                  {statusLabels[section.status]}
                </span>
              </div>

              {/* Diff block */}
              {section.lines.length > 0 && (
                <div className="font-mono text-xs leading-relaxed rounded-md border border-theme-border overflow-hidden">
                  {section.lines.map((line, lineIdx) => (
                    <div
                      key={lineIdx}
                      className="px-3 py-px whitespace-pre"
                      style={
                        line.type === 'added'
                          ? { background: 'rgba(21, 101, 192, 0.10)', color: 'var(--accent-link-hover)' }
                          : line.type === 'removed'
                            ? { background: 'rgba(191, 54, 12, 0.10)', color: 'var(--brand-rust)' }
                            : { background: 'var(--app-bg)', color: 'var(--text-muted)' }
                      }
                    >
                      <span
                        className="inline-block w-4 text-right mr-3 select-none"
                        style={{ color: 'var(--text-muted)', opacity: 0.5 }}
                      >
                        {line.type === 'added' ? '+' : line.type === 'removed' ? '-' : ' '}
                      </span>
                      {line.content}
                    </div>
                  ))}
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
