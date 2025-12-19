# Pivot UX Design

Split button in header to pivot between views while keeping the same process and time range.

## Mockups

- `mockup-header-pivot.html` - From Metrics view (primary: Show Performance, dropdown: Show Log)
- `mockup-header-pivot-from-logs.html` - From Log view (primary: Show Metrics, dropdown: Show Performance)

## Design

A split button in the header (left of time range picker):
- **Main button**: One-click pivot to the primary related view
- **Dropdown arrow**: Reveals secondary pivot option

### From Metrics
- Main button: "Log"
- Dropdown: "Performance Analytics"

### From Log
- Main button: "Metrics"
- Dropdown: "Performance Analytics"

### From Performance Analytics
- Main button: "Log"
- Dropdown: "Metrics"

## Implementation Notes

Pivot preserves `process_id` and current time range:
- Log: `/process_log?process_id=X&begin=Y&end=Z`
- Performance: `/performance_analysis?process_id=X&begin=Y&end=Z`
- Metrics: `/process_metrics?process_id=X&begin=Y&end=Z`
