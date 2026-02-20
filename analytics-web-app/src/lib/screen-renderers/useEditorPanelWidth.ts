import { useState, useCallback, useEffect } from 'react'

const EDITOR_PANEL_MIN_WIDTH = 280
const EDITOR_PANEL_MAX_WIDTH = 800
const EDITOR_PANEL_DEFAULT_WIDTH = 350

interface UseEditorPanelWidthResult {
  editorPanelWidth: number
  handleEditorPanelResize: (delta: number) => void
}

/**
 * Manages editor panel width with localStorage persistence.
 *
 * Reads the initial width from localStorage and persists changes back.
 * Clamps the width between min and max bounds on resize.
 */
export function useEditorPanelWidth(): UseEditorPanelWidthResult {
  const [editorPanelWidth, setEditorPanelWidth] = useState(() => {
    const saved = localStorage.getItem('notebook-editor-panel-width')
    return saved ? parseInt(saved, 10) : EDITOR_PANEL_DEFAULT_WIDTH
  })

  useEffect(() => {
    localStorage.setItem('notebook-editor-panel-width', String(editorPanelWidth))
  }, [editorPanelWidth])

  const handleEditorPanelResize = useCallback((delta: number) => {
    setEditorPanelWidth((prev) =>
      Math.max(EDITOR_PANEL_MIN_WIDTH, Math.min(EDITOR_PANEL_MAX_WIDTH, prev - delta))
    )
  }, [])

  return { editorPanelWidth, handleEditorPanelResize }
}
