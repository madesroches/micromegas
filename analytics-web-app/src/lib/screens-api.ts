import { authenticatedFetch, getApiBase } from './api'

// Types matching the backend models

/**
 * Screen config base interface with common fields shared by all screen types.
 * Each renderer may cast this to a more specific interface (e.g., NotebookConfig).
 * The backend stores this as JSON and passes it through without interpretation.
 *
 * Time range fields are included here since they're used by all screen types
 * for delta-based URL handling (URL only contains values differing from saved config).
 */
export interface ScreenConfig {
  timeRangeFrom?: string
  timeRangeTo?: string
  /** Index signature allows renderer-specific fields */
  [key: string]: unknown
}

export interface ScreenVariable {
  name: string
  default_value?: string
}

export interface Screen {
  name: string
  screen_type: ScreenTypeName
  config: ScreenConfig
  created_by?: string
  updated_by?: string
  created_at: string
  updated_at: string
}

export type ScreenTypeName = 'process_list' | 'metrics' | 'log' | 'table' | 'notebook'

export interface ScreenTypeInfo {
  name: ScreenTypeName
  display_name: string
  icon: string
  description: string
}

export interface CreateScreenRequest {
  name: string
  screen_type: ScreenTypeName
  config: ScreenConfig
}

export interface UpdateScreenRequest {
  config: ScreenConfig
}

export interface ApiErrorResponse {
  code: string
  message: string
}

export class ScreenApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public status: number
  ) {
    super(message)
    this.name = 'ScreenApiError'
  }
}

async function handleResponse<T>(response: Response): Promise<T> {
  if (!response.ok) {
    let errorData: ApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new ScreenApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
  return response.json()
}

// Screen Types API

export async function getScreenTypes(): Promise<ScreenTypeInfo[]> {
  const response = await authenticatedFetch(`${getApiBase()}/screen-types`)
  return handleResponse<ScreenTypeInfo[]>(response)
}

export async function getDefaultConfig(typeName: ScreenTypeName): Promise<ScreenConfig> {
  const response = await authenticatedFetch(`${getApiBase()}/screen-types/${typeName}/default`)
  return handleResponse<ScreenConfig>(response)
}

// Screens CRUD API

export async function listScreens(): Promise<Screen[]> {
  const response = await authenticatedFetch(`${getApiBase()}/screens`)
  return handleResponse<Screen[]>(response)
}

export async function getScreen(name: string): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`)
  return handleResponse<Screen>(response)
}

export async function createScreen(request: CreateScreenRequest): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })
  return handleResponse<Screen>(response)
}

export async function updateScreen(name: string, request: UpdateScreenRequest): Promise<Screen> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`, {
    method: 'PUT',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify(request),
  })
  return handleResponse<Screen>(response)
}

export async function deleteScreen(name: string): Promise<void> {
  const response = await authenticatedFetch(`${getApiBase()}/screens/${encodeURIComponent(name)}`, {
    method: 'DELETE',
  })
  if (!response.ok) {
    let errorData: ApiErrorResponse | undefined
    try {
      errorData = await response.json()
    } catch {
      // Ignore JSON parse errors
    }
    throw new ScreenApiError(
      errorData?.code ?? 'UNKNOWN_ERROR',
      errorData?.message ?? `HTTP ${response.status}`,
      response.status
    )
  }
}

// Export / Import types and helpers

export interface ExportedScreen {
  name: string
  screen_type: ScreenTypeName
  config: ScreenConfig
}

export interface ScreensExportFile {
  version: number
  exported_at: string
  screens: ExportedScreen[]
}

export type ImportConflictAction = 'skip' | 'overwrite' | 'rename'

export interface ImportScreenResult {
  name: string
  status: 'created' | 'skipped' | 'overwritten' | 'renamed' | 'error'
  finalName?: string
  error?: string
}

export function buildScreensExport(screens: Screen[]): string {
  const exported: ScreensExportFile = {
    version: 1,
    exported_at: new Date().toISOString(),
    screens: screens.map((s) => ({
      name: s.name,
      screen_type: s.screen_type,
      config: s.config,
    })),
  }
  return JSON.stringify(exported, null, 2)
}

export function parseScreensImportFile(json: string): ScreensExportFile {
  let parsed: unknown
  try {
    parsed = JSON.parse(json)
  } catch {
    throw new Error('Invalid JSON file')
  }

  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error('Invalid export file: expected a JSON object')
  }

  const obj = parsed as Record<string, unknown>

  if (typeof obj.version !== 'number') {
    throw new Error('Invalid export file: missing "version" field')
  }

  if (!Array.isArray(obj.screens)) {
    throw new Error('Invalid export file: missing "screens" array')
  }

  for (const screen of obj.screens) {
    if (typeof screen !== 'object' || screen === null) {
      throw new Error('Invalid export file: each screen must be an object')
    }
    const s = screen as Record<string, unknown>
    if (typeof s.name !== 'string' || typeof s.screen_type !== 'string' || typeof s.config !== 'object' || s.config === null) {
      throw new Error(`Invalid export file: screen is missing required fields (name, screen_type, config)`)
    }
  }

  return {
    version: obj.version as number,
    exported_at: (obj.exported_at as string) ?? '',
    screens: obj.screens as ExportedScreen[],
  }
}

export function generateUniqueName(baseName: string, existingNames: Set<string>): string {
  const candidate = `${baseName}-imported`
  if (!existingNames.has(candidate)) {
    return candidate
  }
  let counter = 2
  while (existingNames.has(`${baseName}-imported-${counter}`)) {
    counter++
  }
  return `${baseName}-imported-${counter}`
}

export async function importScreen(
  screen: ExportedScreen,
  onConflict: ImportConflictAction,
  existingNames: Set<string>
): Promise<ImportScreenResult> {
  const isConflict = existingNames.has(screen.name)

  if (!isConflict) {
    await createScreen({
      name: screen.name,
      screen_type: screen.screen_type,
      config: screen.config,
    })
    return { name: screen.name, status: 'created' }
  }

  switch (onConflict) {
    case 'skip':
      return { name: screen.name, status: 'skipped' }

    case 'overwrite':
      await updateScreen(screen.name, { config: screen.config })
      return { name: screen.name, status: 'overwritten' }

    case 'rename': {
      const newName = generateUniqueName(screen.name, existingNames)
      await createScreen({
        name: newName,
        screen_type: screen.screen_type,
        config: screen.config,
      })
      return { name: screen.name, status: 'renamed', finalName: newName }
    }
  }
}
