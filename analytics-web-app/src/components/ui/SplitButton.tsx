'use client'

import * as React from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { ChevronDown } from 'lucide-react'
import { cn } from '@/lib/utils'

export interface SplitButtonAction {
  label: string
  icon?: React.ReactNode
  onClick: () => void
}

export interface SplitButtonProps {
  primaryLabel: string
  primaryIcon?: React.ReactNode
  onPrimaryClick: () => void
  secondaryActions: SplitButtonAction[]
  disabled?: boolean
  loading?: boolean
  loadingLabel?: string
  className?: string
}

export function SplitButton({
  primaryLabel,
  primaryIcon,
  onPrimaryClick,
  secondaryActions,
  disabled = false,
  loading = false,
  loadingLabel,
  className,
}: SplitButtonProps) {
  const isDisabled = disabled || loading
  const displayLabel = loading && loadingLabel ? loadingLabel : primaryLabel

  return (
    <div className={cn('inline-flex rounded-md', className)}>
      {/* Primary button */}
      <button
        onClick={onPrimaryClick}
        disabled={isDisabled}
        className="flex items-center gap-2 px-4 py-2 bg-accent-link text-white rounded-l-md hover:bg-accent-link-hover disabled:bg-theme-border disabled:text-theme-text-muted disabled:cursor-not-allowed transition-colors text-sm font-medium"
      >
        {loading ? (
          <div className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
        ) : (
          primaryIcon
        )}
        {displayLabel}
      </button>

      {/* Dropdown trigger */}
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            disabled={isDisabled}
            className="flex items-center px-2 py-2 bg-accent-link text-white rounded-r-md border-l border-accent-link-hover hover:bg-accent-link-hover disabled:bg-theme-border disabled:text-theme-text-muted disabled:cursor-not-allowed transition-colors"
            aria-label="More options"
          >
            <ChevronDown className="w-4 h-4" />
          </button>
        </DropdownMenu.Trigger>

        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            className="min-w-[160px] bg-app-panel border border-theme-border rounded-md shadow-lg py-1 z-50"
          >
            {secondaryActions.map((action, index) => (
              <DropdownMenu.Item
                key={index}
                onClick={action.onClick}
                className="flex items-center gap-2 px-3 py-2 text-sm text-theme-text-primary hover:bg-theme-border/50 cursor-pointer outline-none"
              >
                {action.icon}
                {action.label}
              </DropdownMenu.Item>
            ))}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )
}
