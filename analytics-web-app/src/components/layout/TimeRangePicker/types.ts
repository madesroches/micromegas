/**
 * Props for the TimeRangePicker component.
 * Follows the controlled component pattern - receives values and onChange from parent.
 */
export interface TimeRangePickerProps {
  /** Raw "from" value (relative like "now-1h" or ISO string) */
  from: string
  /** Raw "to" value (relative like "now" or ISO string) */
  to: string
  /** Callback when user selects a new time range */
  onChange: (from: string, to: string) => void
}

export interface QuickRangesProps {
  currentFrom: string
  currentTo: string
  onSelect: (from: string, to: string) => void
}

export interface CustomRangeProps {
  from: string
  to: string
  onApply: (from: string, to: string) => void
}

export interface RecentRangesProps {
  onSelect: (from: string, to: string) => void
}
