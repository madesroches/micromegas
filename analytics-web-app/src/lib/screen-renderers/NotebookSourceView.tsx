import { useState, useCallback, useEffect, useMemo } from 'react'
import { Check, Copy, Pencil } from 'lucide-react'
import type { NotebookConfig } from './notebook-types'

interface NotebookSourceViewProps {
  notebookConfig: NotebookConfig
  onConfigChange: (config: NotebookConfig) => void
  onBack: () => void
}

export function NotebookSourceView({ notebookConfig, onConfigChange, onBack }: NotebookSourceViewProps) {
  const [editingSource, setEditingSource] = useState(false)
  const [sourceText, setSourceText] = useState('')
  const [baselineJson, setBaselineJson] = useState('')
  const [copied, setCopied] = useState(false)

  const jsonError = useMemo(() => {
    if (!editingSource || !sourceText) return null
    try {
      const parsed = JSON.parse(sourceText)
      if (!parsed.cells || !Array.isArray(parsed.cells)) {
        return 'JSON must contain a "cells" array'
      }
      return null
    } catch (e) {
      return e instanceof Error ? e.message : 'Invalid JSON'
    }
  }, [editingSource, sourceText])

  const exitSourceView = useCallback(() => {
    setEditingSource(false)
    setSourceText('')
    onBack()
  }, [onBack])

  const configJson = useMemo(() => JSON.stringify(notebookConfig, null, 2), [notebookConfig])

  const hasUnsavedEdits = editingSource && sourceText !== baselineJson

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (hasUnsavedEdits) {
          if (!window.confirm('Discard unsaved changes?')) return
        }
        exitSourceView()
      }
    }
    document.addEventListener('keydown', handleKeyDown)
    return () => document.removeEventListener('keydown', handleKeyDown)
  }, [hasUnsavedEdits, exitSourceView])

  return (
    <div className="flex flex-col gap-4 h-full">
      <div className="flex items-center gap-3">
        <button
          onClick={() => {
            if (hasUnsavedEdits && !window.confirm('Discard unsaved changes?')) return
            exitSourceView()
          }}
          className="text-sm text-accent-link hover:underline"
        >
          &larr; Back to notebook
        </button>
        <span className="text-[11px] px-1.5 py-0.5 rounded bg-app-card text-theme-text-secondary font-mono font-medium">
          JSON
        </span>
        <span className="text-sm text-theme-text-primary font-medium">Notebook Configuration</span>
        <span className="text-xs text-theme-text-muted">{editingSource ? 'editing' : 'read-only'}</span>
        <div className="flex-1" />
        <button
          onClick={async () => {
            const text = editingSource ? sourceText : configJson
            try {
              await navigator.clipboard.writeText(text)
              setCopied(true)
              setTimeout(() => setCopied(false), 2000)
            } catch {
              // ignore clipboard errors
            }
          }}
          className="p-1.5 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
          title="Copy to clipboard"
        >
          {copied ? <Check className="w-4 h-4 text-green-600" /> : <Copy className="w-4 h-4" />}
        </button>
        {editingSource ? (
          <>
            <button
              onClick={() => {
                if (hasUnsavedEdits && !window.confirm('Discard unsaved changes?')) return
                setEditingSource(false)
                setSourceText('')
              }}
              className="px-3 py-1 text-xs rounded border border-theme-border text-theme-text-secondary hover:text-theme-text-primary hover:bg-app-card transition-colors"
            >
              Cancel
            </button>
            <button
              disabled={!!jsonError}
              onClick={() => {
                onConfigChange(JSON.parse(sourceText))
                onBack()
              }}
              className="px-3 py-1 text-xs rounded bg-accent-link text-white hover:bg-accent-link/80 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              Apply
            </button>
          </>
        ) : (
          <button
            onClick={() => {
              setEditingSource(true)
              setSourceText(configJson)
              setBaselineJson(configJson)
            }}
            className="p-1.5 text-theme-text-muted hover:text-theme-text-primary rounded transition-colors"
            title="Edit source"
          >
            <Pencil className="w-4 h-4" />
          </button>
        )}
      </div>
      {editingSource ? (
        <div className="flex flex-col gap-2 flex-1 min-h-0">
          <textarea
            value={sourceText}
            onChange={(e) => setSourceText(e.target.value)}
            className="bg-app-card border border-theme-border rounded-lg p-4 overflow-auto text-xs font-mono text-theme-text-secondary whitespace-pre flex-1 min-h-0 w-full focus:outline-none focus:border-accent-link"
            spellCheck={false}
          />
          {jsonError && (
            <div className="px-3 py-2 bg-accent-error/10 border border-accent-error/30 rounded text-xs text-accent-error shrink-0">
              {jsonError}
            </div>
          )}
        </div>
      ) : (
        <pre className="bg-app-card border border-theme-border rounded-lg p-4 overflow-auto text-xs font-mono text-theme-text-secondary whitespace-pre flex-1 min-h-0">
          {configJson}
        </pre>
      )}
    </div>
  )
}
