/**
 * Shared pagination hook and UI component for notebook cells (Table, Log).
 *
 * Page size is persisted via cell options; current page is ephemeral component state.
 */

/* eslint-disable react-refresh/only-export-components */

import { useState, useCallback, useEffect, useMemo } from 'react'
import {
  ChevronsLeft,
  ChevronLeft,
  ChevronRight,
  ChevronsRight,
} from 'lucide-react'

// =============================================================================
// Constants
// =============================================================================

export const DEFAULT_PAGE_SIZE = 100
export const PAGE_SIZE_OPTIONS = [50, 100, 250, 500, 1000]

// =============================================================================
// Hook
// =============================================================================

export interface PaginationState {
  currentPage: number
  pageSize: number
  totalRows: number
  totalPages: number
  startRow: number
  endRow: number
  setPage: (page: number) => void
  setPageSize: (size: number) => void
}

export function usePagination(
  totalRows: number,
  pageSize: number,
  onPageSizeChange: (size: number) => void,
): PaginationState {
  const [currentPage, setCurrentPage] = useState(0)

  const totalPages = Math.max(1, Math.ceil(totalRows / pageSize))

  // Clamp page when totalRows or pageSize changes
  useEffect(() => {
    setCurrentPage((prev) => {
      const maxPage = Math.max(0, Math.ceil(totalRows / pageSize) - 1)
      return prev > maxPage ? maxPage : prev
    })
  }, [totalRows, pageSize])

  const startRow = currentPage * pageSize
  const endRow = Math.min(startRow + pageSize, totalRows)

  const setPage = useCallback(
    (page: number) => {
      const clamped = Math.max(0, Math.min(page, totalPages - 1))
      setCurrentPage(clamped)
    },
    [totalPages],
  )

  const setPageSize = useCallback(
    (size: number) => {
      setCurrentPage(0)
      onPageSizeChange(size)
    },
    [onPageSizeChange],
  )

  return useMemo(
    () => ({
      currentPage,
      pageSize,
      totalRows,
      totalPages,
      startRow,
      endRow,
      setPage,
      setPageSize,
    }),
    [currentPage, pageSize, totalRows, totalPages, startRow, endRow, setPage, setPageSize],
  )
}

// =============================================================================
// Component
// =============================================================================

export interface PaginationBarProps {
  pagination: PaginationState
}

export function PaginationBar({ pagination }: PaginationBarProps) {
  const { currentPage, totalPages, totalRows, startRow, endRow, setPage, setPageSize, pageSize } =
    pagination

  if (totalRows === 0 || totalPages <= 1) return null

  const isFirst = currentPage === 0
  const isLast = currentPage >= totalPages - 1

  return (
    <div className="flex items-center justify-between px-3 py-1.5 bg-app-card border-t border-theme-border flex-shrink-0">
      {/* Navigation */}
      <div className="flex items-center gap-0.5">
        <NavButton onClick={() => setPage(0)} disabled={isFirst} title="First page">
          <ChevronsLeft className="w-3.5 h-3.5" />
        </NavButton>
        <NavButton onClick={() => setPage(currentPage - 1)} disabled={isFirst} title="Previous page">
          <ChevronLeft className="w-3.5 h-3.5" />
        </NavButton>
        <span className="text-xs text-theme-text-secondary mx-2 whitespace-nowrap select-none">
          Page{' '}
          <span className="text-theme-text-primary font-semibold">{currentPage + 1}</span>
          {' '}of{' '}
          <span className="text-theme-text-primary font-semibold">
            {totalPages.toLocaleString()}
          </span>
        </span>
        <NavButton onClick={() => setPage(currentPage + 1)} disabled={isLast} title="Next page">
          <ChevronRight className="w-3.5 h-3.5" />
        </NavButton>
        <NavButton onClick={() => setPage(totalPages - 1)} disabled={isLast} title="Last page">
          <ChevronsRight className="w-3.5 h-3.5" />
        </NavButton>
      </div>

      {/* Row info + page size */}
      <div className="flex items-center gap-3">
        <span className="text-[11px] text-theme-text-muted whitespace-nowrap select-none">
          Rows {(startRow + 1).toLocaleString()}&ndash;{endRow.toLocaleString()} of{' '}
          {totalRows.toLocaleString()}
        </span>
        <select
          value={pageSize}
          onChange={(e) => setPageSize(Number(e.target.value))}
          className="text-[11px] px-1.5 py-1 bg-app-bg text-theme-text-secondary border border-theme-border rounded cursor-pointer outline-none hover:border-theme-border-hover focus:border-accent-link"
        >
          {PAGE_SIZE_OPTIONS.map((size) => (
            <option key={size} value={size}>
              {size}
            </option>
          ))}
        </select>
        <span className="text-[11px] text-theme-text-muted select-none">rows/page</span>
      </div>
    </div>
  )
}

// =============================================================================
// Internal
// =============================================================================

function NavButton({
  onClick,
  disabled,
  title,
  children,
}: {
  onClick: () => void
  disabled: boolean
  title: string
  children: React.ReactNode
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      title={title}
      className="w-7 h-7 flex items-center justify-center border border-theme-border rounded text-theme-text-secondary transition-colors hover:bg-theme-border/50 hover:text-theme-text-primary hover:border-theme-border-hover disabled:opacity-30 disabled:cursor-default disabled:hover:bg-transparent disabled:hover:text-theme-text-secondary disabled:hover:border-theme-border"
    >
      {children}
    </button>
  )
}
