import { List, LineChart, FileText, type LucideIcon } from 'lucide-react'

/**
 * Maps icon identifiers from the backend to Lucide React components.
 * When adding a new screen type with a new icon, add it here.
 */
const ICON_MAP: Record<string, LucideIcon> = {
  list: List,
  'chart-line': LineChart,
  'file-text': FileText,
}

const DEFAULT_ICON = FileText

/**
 * Returns the Lucide icon component for the given icon identifier.
 */
export function getIconComponent(iconName: string): LucideIcon {
  return ICON_MAP[iconName] ?? DEFAULT_ICON
}

/**
 * Renders an icon element for the given icon identifier.
 */
export function renderIcon(iconName: string, className = 'w-5 h-5') {
  const Icon = getIconComponent(iconName)
  return <Icon className={className} />
}
