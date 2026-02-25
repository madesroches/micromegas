# Analytics Web App

The analytics web app is a React single-page application backed by a Rust HTTP server. It provides a browser-based interface for querying and visualizing observability data stored in the Micromegas data lake.

The backend (`analytics-web-srv`) proxies SQL queries to the FlightSQL analytics service and handles authentication via OIDC. The frontend renders query results as tables, charts, logs, and more.

## Screens

Screens are the primary unit of saved state in the web app. Each screen has a name, a type, and a configuration that determines what data is displayed and how.

### Screen Types

| Type | Description |
|------|-------------|
| **Notebook** | Multi-cell canvas combining SQL queries, charts, tables, markdown, and variables. The primary screen type. |
| **Process List** | Tabular list of processes with sortable columns. |
| **Metrics** | Time-series chart from a SQL query. |
| **Log** | Log entry viewer with level coloring. |
| **Table** | SQL query results in a sortable table. |

!!! tip
    Notebooks can replicate all the functionality of the built-in screen types (process list, metrics, log, table) with greater flexibility. New screens should generally be created as notebooks.

### Creating a Screen

1. Navigate to **Screens** in the sidebar.
2. Click the **+** button.
3. Choose a screen type (typically **Notebook**).
4. Configure the screen and click **Save**.

Saved screens appear in the screen list and can be shared by URL.

## Time Range

Every screen that displays time-series data uses a global time range. The time range picker in the header supports both relative and absolute ranges.

### Relative Ranges

Relative ranges are evaluated at query time:

- `now-5m` (last 5 minutes)
- `now-1h` (last hour)
- `now-7d` (last 7 days)

Quick presets are available for common durations from 5 minutes to 90 days.

### Absolute Ranges

Pick specific start and end times using the custom range date/time pickers.

### URL Parameters

Time range is stored in the URL as `from` and `to` parameters:

```
/screen/my-dashboard?from=now-1h&to=now
```

This means sharing a URL shares the exact time range. After saving a screen, URL parameters that match the saved defaults are automatically cleaned up.

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `t` | Open time range picker |
| `Ctrl+Shift+C` | Copy time range to clipboard |
| `Ctrl+Shift+V` | Paste time range from clipboard |

## Data Sources

Data sources define which FlightSQL analytics service to query. A default data source is configured at the system level. Individual screens and notebook cells can override the data source to query different backends.

Data sources are managed from the **Admin** page.

## Further Reading

- [Notebooks](notebooks/index.md) — the primary screen type for building interactive dashboards
- [Deployment](../admin/web-app.md) — server configuration and deployment guide
