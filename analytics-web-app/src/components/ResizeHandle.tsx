import { useCallback, useRef } from 'react'

interface ResizeHandleProps {
  onResize: (deltaY: number) => void
  minHeight?: number
}

export function ResizeHandle({ onResize, minHeight = 50 }: ResizeHandleProps) {
  const startY = useRef<number>(0)
  const isDragging = useRef(false)

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()
      startY.current = e.clientY
      isDragging.current = true

      const handleMouseMove = (moveEvent: MouseEvent) => {
        if (!isDragging.current) return
        const deltaY = moveEvent.clientY - startY.current
        startY.current = moveEvent.clientY
        onResize(deltaY)
      }

      const handleMouseUp = () => {
        isDragging.current = false
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = 'ns-resize'
      document.body.style.userSelect = 'none'
    },
    [onResize]
  )

  return (
    <div
      className="h-3 cursor-ns-resize flex items-center justify-center group"
      onMouseDown={handleMouseDown}
      role="separator"
      aria-orientation="horizontal"
      aria-valuenow={minHeight}
    >
      <div className="w-16 h-1 rounded-full bg-theme-border group-hover:bg-accent-link transition-colors" />
    </div>
  )
}
