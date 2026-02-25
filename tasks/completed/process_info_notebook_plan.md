# objective
design a notebook screen - add log, metrics and performance analysis links like in the process info built-in page
time range should be clamped in the same way as the process built-in page.

# hints

 - tables and transposed tables can have md overrides with link
 - md cells can contain links and use variables
 - queries can run against the notebook cells as tables

# source
{
  "cells": [
    {
      "sql": "SELECT DISTINCT name FROM measures",
      "name": "source",
      "type": "variable",
      "layout": {
        "height": 0
      },
      "dataSource": "test",
      "defaultValue": "prod",
      "variableType": "datasource"
    },
    {
      "sql": "SELECT DISTINCT name FROM measures",
      "name": "process_id",
      "type": "variable",
      "layout": {
        "height": 0
      },
      "dataSource": "test",
      "variableType": "text",
      "autoRunFromHere": true
    },
    {
      "sql": "SELECT \n  exe,\n  username, \n  computer, \n  distro, \n  cpu_brand, \n  start_time, \n  last_update_time,\n  properties\nFROM processes\nWHERE process_id = '$process_id'",
      "name": "process_info",
      "type": "transposed",
      "layout": {
        "height": 294
      },
      "options": {
        "hiddenRows": [
          "properties"
        ]
      },
      "dataSource": "$source"
    },
    {
      "sql": "SELECT key as name, jsonb_as_string(value) as value\nFROM jsonb_each((SELECT properties FROM process_info))\n",
      "name": "properties",
      "type": "table",
      "layout": {
        "height": 462
      },
      "dataSource": "notebook"
    }
  ],
  "timeRangeTo": "now",
  "timeRangeFrom": "now-1h"
}


# solution
```json
{
  "cells": [
    {
      "sql": "SELECT DISTINCT name FROM measures",
      "name": "source",
      "type": "variable",
      "layout": {
        "height": 0
      },
      "dataSource": "test",
      "defaultValue": "prod",
      "variableType": "datasource"
    },
    {
      "sql": "SELECT DISTINCT name FROM measures",
      "name": "process_id",
      "type": "variable",
      "layout": {
        "height": 0
      },
      "dataSource": "test",
      "variableType": "text",
      "autoRunFromHere": true
    },
    {
      "sql": "SELECT \n  exe,\n  username, \n  computer, \n  distro, \n  cpu_brand, \n  start_time, \n  last_update_time,\n  properties,\n  '' as links,\n  CASE\n    WHEN last_update_time > now() - INTERVAL '2 minutes' THEN\n      CASE WHEN start_time > now() - INTERVAL '1 hour' THEN start_time ELSE now() - INTERVAL '1 hour' END\n    ELSE\n      CASE WHEN start_time > last_update_time - INTERVAL '1 hour' THEN start_time ELSE last_update_time - INTERVAL '1 hour' END\n  END as range_from,\n  CASE\n    WHEN last_update_time > now() - INTERVAL '2 minutes' THEN now()\n    ELSE last_update_time\n  END as range_to\nFROM processes\nWHERE process_id = '$process_id'",
      "name": "process_info",
      "type": "transposed",
      "layout": {
        "height": 294
      },
      "options": {
        "hiddenRows": [
          "properties",
          "range_from",
          "range_to"
        ],
        "overrides": [
          {
            "column": "links",
            "format": "[View Log](/process_log?process_id=$process_id&from=$row.range_from&to=$row.range_to) · [View Metrics](/process_metrics?process_id=$process_id&from=$row.range_from&to=$row.range_to) · [Performance Analysis](/performance_analysis?process_id=$process_id&from=$row.range_from&to=$row.range_to)"
          }
        ]
      },
      "dataSource": "$source"
    },
    {
      "sql": "SELECT key as name, jsonb_as_string(value) as value\nFROM jsonb_each((SELECT properties FROM process_info))\n",
      "name": "properties",
      "type": "table",
      "layout": {
        "height": 462
      },
      "dataSource": "notebook"
    }
  ],
  "timeRangeTo": "now",
  "timeRangeFrom": "now-1h"
}
```

## How it works

The `process_info` transposed table is modified to include:

1. **Clamped time range columns** (`range_from`, `range_to`) computed with the same logic as `ProcessPage.tsx`:
   - **Live detection**: If `last_update_time > now() - 2 minutes`, the process is considered live and `range_to = now()`; otherwise `range_to = last_update_time`
   - **From clamping**: `range_from = max(start_time, end_time - 1 hour)` — never extends before the process started, defaults to 1-hour window

2. **A `links` placeholder column** overridden with markdown to render three clickable links: View Log, View Metrics, Performance Analysis

3. **Hidden rows**: `properties`, `range_from`, `range_to` are hidden from display — they exist only to support the override macros and the downstream properties cell

The override uses `$process_id` (notebook variable) and `$row.range_from` / `$row.range_to` (row data with automatic ISO timestamp formatting for URL compatibility).
