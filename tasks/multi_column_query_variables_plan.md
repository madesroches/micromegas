# Multi-Column Query Variables with Row.Column Substitution

## Status: IMPLEMENTED

## Issue Reference
- GitHub Issue: [#747](https://github.com/madesroches/micromegas/issues/747)

## Overview

Enhance query variables to support multi-column results with `$variable.column` macro substitution syntax. This allows users to define a single query variable that returns multiple related values (e.g., metric name and unit), then reference specific columns throughout the notebook.

**Example Use Case:**
```sql
-- Query variable: selected_metric
SELECT name, unit FROM available_metrics WHERE category = 'performance'
```

Then in cells:
```sql
SELECT time, value FROM metrics WHERE name = '$selected_metric.name'
```

And in chart Y-axis unit: `$selected_metric.unit`

## Current Architecture

### Variable Storage
- **File:** `analytics-web-app/src/lib/screen-renderers/useNotebookVariables.ts`
- Variables stored as `Record<string, string>` - simple key-value strings
- Values synchronized with URL parameters using delta encoding

### Macro Substitution
- **File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`
- `substituteMacros()` replaces `$variableName` with string values
- Uses word boundary regex: `\$${name}\b`
- Sorts by name length to avoid partial matches

### Variable Cell Execution
- **File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`
- Combobox variables execute SQL to populate options
- Extracts options: 1 column = value+label, 2 columns = value (col 1) and label (col 2)
- **Current limitation:** Only the first column value is used as the variable value

### Chart Properties
- **File:** `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx`
- Options stored in `config.options` as `Record<string, unknown>`
- **Current limitation:** No macro substitution in chart options

## Implementation Plan

### Phase 1: Data Model Changes

#### 1.1 Update Variable Value Type
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-types.ts`

Add a new type for multi-column variable values:

```typescript
// A variable can be a simple string or an object with column values
export type VariableValue = string | Record<string, string>

// Keep the existing interface for simple access, add helper function
export function getVariableString(value: VariableValue): string {
  if (typeof value === 'string') return value
  // For multi-column values, return the first column value (or empty string)
  const keys = Object.keys(value)
  return keys.length > 0 ? value[keys[0]] : ''
}
```

#### 1.2 Update Variable Storage
**File:** `analytics-web-app/src/lib/screen-renderers/useNotebookVariables.ts`

Change the storage type from `Record<string, string>` to `Record<string, VariableValue>`:

```typescript
export interface UseNotebookVariablesResult {
  variableValues: Record<string, VariableValue>  // Changed from string
  variableValuesRef: React.MutableRefObject<Record<string, VariableValue>>
  setVariableValue: (cellName: string, value: VariableValue) => void
  // ... rest unchanged
}
```

**URL Encoding:** For multi-column values, serialize as JSON in URL parameters:
- Simple string: `?myVar=value`
- Multi-column: `?myVar={"name":"cpu","unit":"percent"}`

Update `encodeVariableValue()` and `decodeVariableValue()` helper functions.

### Phase 2: Macro Substitution Enhancement

#### 2.1 Update substituteMacros Function
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

Enhance `substituteMacros()` to handle both simple and dotted variable references:

```typescript
export function substituteMacros(
  sql: string,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string }
): string {
  let result = sql

  // 1. Handle time range macros (unchanged)
  result = result.replace(/\$begin\b/g, `'${timeRange.begin}'`)
  result = result.replace(/\$end\b/g, `'${timeRange.end}'`)

  // 2. Handle dotted variable references first: $variable.column
  //    Must process before simple variables to avoid partial matches
  const dottedPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  result = result.replace(dottedPattern, (match, varName, colName) => {
    const value = variables[varName]
    if (value === undefined) return match  // Leave unresolved

    if (typeof value === 'string') {
      // Simple variable doesn't have columns - return empty or error marker
      console.warn(`Variable '${varName}' is not a multi-column variable, cannot access '${colName}'`)
      return ''  // or return `/*INVALID: ${match}*/` for debugging
    }

    const colValue = value[colName]
    if (colValue === undefined) {
      console.warn(`Column '${colName}' not found in variable '${varName}'`)
      return ''
    }

    return escapeSqlValue(colValue)
  })

  // 3. Handle simple variable references: $variable
  //    Sort by name length descending to avoid partial matches
  const sortedNames = Object.keys(variables).sort((a, b) => b.length - a.length)

  for (const name of sortedNames) {
    const value = variables[name]
    const regex = new RegExp(`\\$${name}\\b`, 'g')

    if (typeof value === 'string') {
      result = result.replace(regex, escapeSqlValue(value))
    } else {
      // Multi-column variable referenced without column - use first column value
      const firstKey = Object.keys(value)[0]
      const firstValue = firstKey ? value[firstKey] : ''
      result = result.replace(regex, escapeSqlValue(firstValue))
    }
  }

  return result
}

function escapeSqlValue(value: string): string {
  return value.replace(/'/g, "''")
}
```

#### 2.2 Add Validation Helper
**File:** `analytics-web-app/src/lib/screen-renderers/notebook-utils.ts`

Add validation for macro references:

```typescript
export interface MacroValidationResult {
  valid: boolean
  errors: string[]
}

export function validateMacros(
  text: string,
  variables: Record<string, VariableValue>
): MacroValidationResult {
  const errors: string[] = []

  // Check dotted references
  const dottedPattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\.([a-zA-Z_][a-zA-Z0-9_]*)\b/g
  let match
  while ((match = dottedPattern.exec(text)) !== null) {
    const [fullMatch, varName, colName] = match
    const value = variables[varName]

    if (value === undefined) {
      errors.push(`Unknown variable: ${varName}`)
    } else if (typeof value === 'string') {
      errors.push(`Variable '${varName}' is not a multi-column variable, cannot access '${colName}'`)
    } else if (value[colName] === undefined) {
      errors.push(`Column '${colName}' not found in variable '${varName}'. Available: ${Object.keys(value).join(', ')}`)
    }
  }

  // Check simple variable references
  const simplePattern = /\$([a-zA-Z_][a-zA-Z0-9_]*)\b(?!\.)/g
  while ((match = simplePattern.exec(text)) !== null) {
    const [, varName] = match
    if (varName !== 'begin' && varName !== 'end' && varName !== 'order_by') {
      if (variables[varName] === undefined) {
        errors.push(`Unknown variable: ${varName}`)
      }
    }
  }

  return { valid: errors.length === 0, errors }
}
```

### Phase 3: Variable Cell Updates

#### 3.1 Update Variable Execution
**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Modify the execution to capture all columns:

```typescript
execute: async (config: CellConfig, { variables, timeRange, runQuery }: CellExecutionContext) => {
  const varConfig = config as VariableCellConfig

  if (varConfig.variableType !== 'combobox' || !varConfig.sql) {
    return null
  }

  const sql = substituteMacros(varConfig.sql, variables, timeRange)
  const result = await runQuery(sql)

  // Extract options with all column values
  const options = extractMultiColumnOptions(result)
  return { data: result, variableOptions: options }
}

interface MultiColumnOption {
  value: VariableValue      // Full row as object, or string for single-column
}

function extractMultiColumnOptions(result: QueryResult): MultiColumnOption[] {
  const schema = result.schema
  const columnNames = schema.fields.map(f => f.name)
  const options: MultiColumnOption[] = []

  for (let i = 0; i < result.numRows; i++) {
    const row = result.get(i)
    if (!row) continue

    if (columnNames.length === 1) {
      // Single column: store as string
      const val = String(row[columnNames[0]] ?? '')
      options.push({ value: val })
    } else {
      // Multiple columns: store entire row as object
      const rowObj: Record<string, string> = {}
      for (const col of columnNames) {
        rowObj[col] = String(row[col] ?? '')
      }
      options.push({ value: rowObj })
    }
  }

  return options
}

// Display helper for dropdown - shows all column values
function formatOptionDisplay(value: VariableValue): string {
  if (typeof value === 'string') return value
  return Object.values(value).join(' | ')
}
```

#### 3.2 Update Variable UI Component
**File:** `analytics-web-app/src/lib/screen-renderers/cells/VariableCell.tsx`

Update the combobox to handle multi-column values:

```typescript
// Serialize/deserialize for comparison and storage
function serializeValue(value: VariableValue): string {
  return typeof value === 'string' ? value : JSON.stringify(value)
}

function deserializeValue(str: string): VariableValue {
  try {
    const parsed = JSON.parse(str)
    if (typeof parsed === 'object' && parsed !== null) {
      return parsed as Record<string, string>
    }
  } catch {
    // Not JSON, return as string
  }
  return str
}

// In the render function
<Select
  value={serializeValue(currentValue)}
  onValueChange={(serialized) => {
    const value = deserializeValue(serialized)
    setVariableValue(cellName, value)
  }}
>
  {options.map((opt) => (
    <SelectItem key={serializeValue(opt.value)} value={serializeValue(opt.value)}>
      {formatOptionDisplay(opt.value)}
    </SelectItem>
  ))}
</Select>
```

### Phase 4: Chart Property Substitution

#### 4.1 Add Macro Substitution to Chart Options
**File:** `analytics-web-app/src/lib/screen-renderers/cells/ChartCell.tsx`

Apply macro substitution to string values in chart options:

```typescript
function substituteOptionsWithMacros(
  options: Record<string, unknown> | undefined,
  variables: Record<string, VariableValue>,
  timeRange: { begin: string; end: string }
): Record<string, unknown> {
  if (!options) return {}

  const result: Record<string, unknown> = {}

  for (const [key, value] of Object.entries(options)) {
    if (typeof value === 'string') {
      // Apply macro substitution to string values
      result[key] = substituteMacros(value, variables, timeRange)
    } else {
      result[key] = value
    }
  }

  return result
}

// In ChartCellRenderer
const ChartCellRenderer: React.FC<CellRendererProps> = ({ config, state, context }) => {
  const chartConfig = config as QueryCellConfig
  const { variables, timeRange } = context

  // Substitute macros in options
  const resolvedOptions = useMemo(
    () => substituteOptionsWithMacros(chartConfig.options, variables, timeRange),
    [chartConfig.options, variables, timeRange]
  )

  // Use resolvedOptions instead of chartConfig.options
  return (
    <XYChart
      // ...
      unit={(resolvedOptions.unit as string) ?? ''}
      scaleMode={(resolvedOptions.scale_mode as ScaleMode) ?? 'p99'}
      chartType={(resolvedOptions.chart_type as ChartType) ?? 'line'}
    />
  )
}
```

#### 4.2 Add Unit Option to Chart Configuration UI
**File:** `analytics-web-app/src/components/ChartOptionsEditor.tsx` (or wherever chart options are edited)

Add a text input for the unit that supports variable substitution:

```typescript
<FormField label="Y-Axis Unit" hint="Use $variable.column for dynamic values">
  <Input
    value={options.unit ?? ''}
    onChange={(e) => updateOption('unit', e.target.value)}
    placeholder="e.g., $selected_metric.unit"
  />
</FormField>
```

### Phase 5: Available Variables Panel Update

#### 5.1 Show Column Information
**File:** `analytics-web-app/src/components/AvailableVariablesPanel.tsx`

Update the panel to show available columns for multi-column variables:

```typescript
interface AvailableVariablesPanelProps {
  variables: Record<string, VariableValue>
  timeRange: { begin: string; end: string }
  additionalVariables?: { name: string; description: string }[]
}

// In render
{Object.entries(variables).map(([name, value]) => (
  <div key={name}>
    {typeof value === 'string' ? (
      <div>${name}: {value}</div>
    ) : (
      <div>
        <div>${name} (multi-column)</div>
        <div className="ml-4 text-muted-foreground">
          {Object.entries(value).map(([col, val]) => (
            <div key={col}>${name}.{col}: {val}</div>
          ))}
        </div>
      </div>
    )}
  </div>
))}
```

### Phase 6: Error Handling

#### 6.1 Show Validation Errors in Editor
**File:** `analytics-web-app/src/components/SqlEditor.tsx` (or cell editor components)

Add inline validation for macro references:

```typescript
const validationResult = useMemo(
  () => validateMacros(sql, variables),
  [sql, variables]
)

{!validationResult.valid && (
  <div className="text-destructive text-sm">
    {validationResult.errors.map((err, i) => (
      <div key={i}>{err}</div>
    ))}
  </div>
)}
```

#### 6.2 Unresolved Macros in Execution
`substituteMacros()` performs best-effort replacement and leaves unresolved macros in place (e.g., `$unknown.col` remains as-is). These flow through to the SQL engine and produce syntax errors, which are handled by existing query error handling:

```typescript
const sql = substituteMacros(config.sql, variables, timeRange)
const result = await runQuery(sql)
// SQL errors (including unresolved macros like "$unknown.col")
// handled by existing query error handling - no special try/catch needed
```

The unresolved macro text in the SQL error message helps users identify the problem.

### Phase 7: Backward Compatibility

#### 7.1 Previous Behavior
The previous implementation handled multi-column queries as:

- **1 column**: value and label are the same string
- **2 columns**: first column = value, second column = label
- **3+ columns**: only first two columns used, rest ignored

#### 7.2 New Behavior
With this enhancement:

- **1 column**: stored as `string`, dropdown displays the value
- **2+ columns**: stored as `Record<string, string>`, dropdown displays all values joined by " | "
- Access individual columns via `$variable.column` syntax
- Access JSON representation via `$variable` (for multi-column values)

#### 7.3 Migration Notes
- Existing notebooks continue to work unchanged
- `$variable` still resolves to the first column value
- No breaking changes - this is purely additive functionality

## File Changes Summary

| File | Change Type | Description |
|------|-------------|-------------|
| `src/lib/screen-renderers/notebook-types.ts` | Modify | Add `VariableValue` type and helper functions |
| `src/lib/screen-renderers/useNotebookVariables.ts` | Modify | Update storage type, URL encoding for objects |
| `src/lib/screen-renderers/notebook-utils.ts` | Modify | Enhance `substituteMacros()` for dotted syntax, add validation |
| `src/lib/screen-renderers/cells/VariableCell.tsx` | Modify | Extract all columns, handle multi-column options |
| `src/lib/screen-renderers/cells/ChartCell.tsx` | Modify | Add macro substitution to chart options |
| `src/components/AvailableVariablesPanel.tsx` | Modify | Display column information for multi-column variables |
| `src/components/ChartOptionsEditor.tsx` | Modify | Add unit input field with variable hint |
| `src/components/SqlEditor.tsx` | Modify | Add macro validation feedback |

## Testing Plan

### Unit Tests
1. `substituteMacros()` with dotted references
2. `substituteMacros()` with simple references to multi-column variables
3. `validateMacros()` error detection
4. URL encoding/decoding of multi-column values
5. Option extraction from multi-column query results

### Integration Tests
1. Create a notebook with a multi-column query variable
2. Reference `$variable.column` in a query cell
3. Verify the query executes with correct substitution
4. Change the variable selection and verify dependent cells re-execute
5. Test chart options with variable substitution

### Manual Testing
1. Create query variable returning `name, unit` columns
2. Use `$var.name` in SQL WHERE clause
3. Use `$var.unit` in chart Y-axis unit option
4. Verify both work correctly
5. Test error messages for invalid column references
6. Test URL sharing with multi-column variable values

## Documentation Updates

### User Guide
Add section on multi-column query variables:
- Creating a query that returns multiple columns
- Using `$variable.column` syntax in SQL
- Using `$variable.column` in chart options
- Error messages and troubleshooting

### Migration Guide
No migration needed - this is additive functionality. Existing notebooks work unchanged.

## Acceptance Criteria Checklist

- [x] Query variables can return multiple columns
- [x] The entire row is stored as the variable value
- [x] Support `$variable.column` syntax for macro substitution in SQL
- [x] Support row.column substitution in chart properties (e.g., unit)
- [x] Provide clear error messages when referencing non-existent columns
- [x] Update Available Variables panel to show column information
- [x] URL state correctly encodes/decodes multi-column values
- [x] Backward compatible with existing single-column variables
- [ ] Update documentation with examples
