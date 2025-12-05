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
