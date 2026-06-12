import React from 'react'
import * as ContextMenu from '@radix-ui/react-context-menu'

export interface LogDividerProps {
  col: string
  pinned: boolean
  hovered: boolean
  onMouseDown: (e: React.MouseEvent) => void
  onContextMenu: (e: React.MouseEvent) => void
  onMouseEnter: () => void
  onMouseLeave: () => void
  onResetToAuto: () => void
  onResetAll: () => void
}

export function LogDivider({
  col,
  pinned,
  hovered,
  onMouseDown,
  onContextMenu,
  onMouseEnter,
  onMouseLeave,
  onResetToAuto,
  onResetAll,
}: LogDividerProps) {
  const lineColor = hovered
    ? 'var(--accent-link)'
    : pinned
      ? 'var(--accent-warning)'
      : 'var(--border-color)'

  return (
    <ContextMenu.Root>
      <ContextMenu.Trigger asChild>
        <span
          data-col={col}
          onMouseDown={onMouseDown}
          onContextMenu={onContextMenu}
          onMouseEnter={onMouseEnter}
          onMouseLeave={onMouseLeave}
          style={{
            display: 'inline-flex',
            alignSelf: 'stretch',
            width: 5,
            minWidth: 5,
            cursor: 'col-resize',
            alignItems: 'center',
            justifyContent: 'center',
            flexShrink: 0,
          }}
        >
          <span
            style={{
              display: 'block',
              width: 1,
              height: '100%',
              backgroundColor: lineColor,
              transition: 'background-color 0.1s',
            }}
          />
        </span>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content
          className="min-w-[160px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50"
        >
          <ContextMenu.Item
            onSelect={onResetToAuto}
            disabled={!pinned}
            className="flex items-center px-3 py-1.5 text-xs text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none data-[disabled]:opacity-40 data-[disabled]:cursor-default"
          >
            Reset to auto
          </ContextMenu.Item>
          <ContextMenu.Item
            onSelect={onResetAll}
            className="flex items-center px-3 py-1.5 text-xs text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
          >
            Reset all columns
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu.Root>
  )
}
