'use client'

import { useState, useCallback } from 'react'
import { ChevronLeft, ChevronRight, Play, RotateCcw } from 'lucide-react'

interface QueryEditorProps {
  defaultSql: string
  variables?: { name: string; description: string }[]
  currentValues?: Record<string, string>
  timeRangeLabel?: string
  onRun: (sql: string) => void
  onReset: () => void
  isLoading?: boolean
  error?: string | null
}

export function QueryEditor({
  defaultSql,
  variables = [],
  currentValues = {},
  timeRangeLabel,
  onRun,
  onReset,
  isLoading = false,
  error,
}: QueryEditorProps) {
  const [isCollapsed, setIsCollapsed] = useState(true)
  const [sql, setSql] = useState(defaultSql)

  const handleRun = useCallback(() => {
    onRun(sql)
  }, [sql, onRun])

  const handleReset = useCallback(() => {
    setSql(defaultSql)
    onReset()
  }, [defaultSql, onReset])

  // Simple SQL syntax highlighting
  const highlightSql = (code: string) => {
    const keywords = /\b(SELECT|FROM|WHERE|AND|OR|ORDER BY|GROUP BY|LIMIT|OFFSET|AS|ON|JOIN|LEFT|RIGHT|INNER|OUTER|DESC|ASC|DISTINCT|COUNT|SUM|AVG|MIN|MAX|CASE|WHEN|THEN|ELSE|END|IN|NOT|NULL|IS|LIKE|BETWEEN)\b/gi
    const strings = /'[^']*'/g
    const variables = /\$[a-z_][a-z0-9_]*/gi

    let result = code
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')

    result = result.replace(strings, '<span class="text-green-400">$&</span>')
    result = result.replace(keywords, '<span class="text-purple-400">$&</span>')
    result = result.replace(variables, '<span class="text-orange-400">$&</span>')

    return result
  }

  if (isCollapsed) {
    return (
      <div className="w-12 bg-[#1a1f26] border-l border-[#2f3540] flex flex-col">
        <div className="p-2">
          <button
            onClick={() => setIsCollapsed(false)}
            className="w-8 h-8 flex items-center justify-center text-gray-400 hover:text-gray-200 hover:bg-[#2f3540] rounded transition-colors"
            title="Expand SQL Panel"
          >
            <ChevronLeft className="w-4 h-4" />
          </button>
        </div>
      </div>
    )
  }

  return (
    <div className="w-96 bg-[#1a1f26] border-l border-[#2f3540] flex flex-col">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 bg-[#22272e] border-b border-[#2f3540]">
        <div className="flex items-center gap-2">
          <button
            onClick={() => setIsCollapsed(true)}
            className="w-6 h-6 flex items-center justify-center text-gray-400 hover:text-gray-200 hover:bg-[#2f3540] rounded transition-colors"
            title="Collapse panel"
          >
            <ChevronRight className="w-4 h-4" />
          </button>
          <span className="text-sm font-semibold text-gray-200">SQL Query</span>
        </div>
        <div className="flex items-center gap-2">
          <button
            onClick={handleReset}
            className="px-2.5 py-1 text-xs text-gray-400 border border-[#2f3540] rounded hover:bg-[#2f3540] hover:text-gray-200 transition-colors"
          >
            Reset
          </button>
          <button
            onClick={handleRun}
            disabled={isLoading}
            className="flex items-center gap-1 px-2.5 py-1 text-xs bg-green-600 text-white rounded hover:bg-green-700 disabled:bg-gray-600 disabled:cursor-not-allowed transition-colors"
          >
            <Play className="w-3 h-3" />
            Run
          </button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-auto p-4">
        {/* SQL Editor */}
        <div className="relative">
          <textarea
            value={sql}
            onChange={(e) => setSql(e.target.value)}
            className="w-full h-48 p-3 bg-[#0d1117] border border-[#2f3540] rounded-md text-gray-200 font-mono text-xs leading-relaxed resize-none focus:outline-none focus:border-blue-500"
            spellCheck={false}
          />
        </div>

        {/* Error */}
        {error && (
          <div className="mt-3 p-3 bg-red-900/20 border border-red-700 rounded-md">
            <p className="text-xs text-red-400">{error}</p>
          </div>
        )}

        {/* Variables */}
        {variables.length > 0 && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-gray-500 mb-2">
              Variables
            </h4>
            <div className="text-xs text-gray-500 space-y-1">
              {variables.map((v) => (
                <div key={v.name}>
                  <code className="px-1.5 py-0.5 bg-[#2f3540] rounded text-orange-400">
                    ${v.name}
                  </code>{' '}
                  - {v.description}
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Current Values */}
        {Object.keys(currentValues).length > 0 && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-gray-500 mb-2">
              Current Values
            </h4>
            <div className="text-xs text-gray-500 space-y-1">
              {Object.entries(currentValues).map(([key, value]) => (
                <div key={key}>
                  <code className="px-1.5 py-0.5 bg-[#2f3540] rounded text-orange-400">
                    ${key}
                  </code>{' '}
                  = <span className="text-gray-300">{value}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {/* Time Range */}
        {timeRangeLabel && (
          <div className="mt-4">
            <h4 className="text-xs font-semibold uppercase tracking-wide text-gray-500 mb-2">
              Time Range
            </h4>
            <p className="text-xs text-gray-500">
              Applied implicitly via FlightSQL headers.
              <br />
              Current: <span className="text-gray-300">{timeRangeLabel}</span>
            </p>
          </div>
        )}
      </div>
    </div>
  )
}
