import { useCallback, useRef } from 'react'

interface ResizeHandleProps {
  onResize: (delta: number) => void
  orientation?: 'horizontal' | 'vertical'
}

export function ResizeHandle({ onResize, orientation = 'vertical' }: ResizeHandleProps) {
  const startPos = useRef<number>(0)
  const isDragging = useRef(false)

  const isHorizontal = orientation === 'horizontal'
  const cursor = isHorizontal ? 'ew-resize' : 'ns-resize'

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()
      startPos.current = isHorizontal ? e.clientX : e.clientY
      isDragging.current = true

      const handleMouseMove = (moveEvent: MouseEvent) => {
        if (!isDragging.current) return
        const currentPos = isHorizontal ? moveEvent.clientX : moveEvent.clientY
        const delta = currentPos - startPos.current
        startPos.current = currentPos
        onResize(delta)
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
      document.body.style.cursor = cursor
      document.body.style.userSelect = 'none'
    },
    [onResize, isHorizontal, cursor]
  )

  if (isHorizontal) {
    return (
      <div
        className="w-1 cursor-ew-resize flex items-center justify-center group hover:bg-accent-link/20 transition-colors"
        onMouseDown={handleMouseDown}
        role="separator"
        aria-orientation="vertical"
      >
        <div className="w-0.5 h-8 rounded-full bg-theme-border group-hover:bg-accent-link transition-colors" />
      </div>
    )
  }

  return (
    <div
      className="h-3 cursor-ns-resize flex items-center justify-center group"
      onMouseDown={handleMouseDown}
      role="separator"
      aria-orientation="horizontal"
    >
      <div className="w-16 h-1 rounded-full bg-theme-border group-hover:bg-accent-link transition-colors" />
    </div>
  )
}
