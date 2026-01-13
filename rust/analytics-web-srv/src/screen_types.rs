use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Error returned when parsing an invalid screen type string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseScreenTypeError {
    invalid_value: String,
}

impl fmt::Display for ParseScreenTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid screen type '{}', expected one of: table, metrics, trace",
            self.invalid_value
        )
    }
}

impl std::error::Error for ParseScreenTypeError {}

/// Types of screens that can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenType {
    Table,
    Metrics,
    Trace,
}

impl FromStr for ScreenType {
    type Err = ParseScreenTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "table" => Ok(ScreenType::Table),
            "metrics" => Ok(ScreenType::Metrics),
            "trace" => Ok(ScreenType::Trace),
            _ => Err(ParseScreenTypeError {
                invalid_value: s.to_string(),
            }),
        }
    }
}

impl ScreenType {
    /// Returns all available screen types.
    pub fn all() -> Vec<ScreenType> {
        vec![ScreenType::Table, ScreenType::Metrics, ScreenType::Trace]
    }

    /// Returns the string identifier for this screen type.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScreenType::Table => "table",
            ScreenType::Metrics => "metrics",
            ScreenType::Trace => "trace",
        }
    }

    /// Returns information about this screen type.
    pub fn info(&self) -> ScreenTypeInfo {
        match self {
            ScreenType::Table => ScreenTypeInfo {
                name: "table".to_string(),
                icon: "table".to_string(),
                description: "SQL query with tabular results".to_string(),
            },
            ScreenType::Metrics => ScreenTypeInfo {
                name: "metrics".to_string(),
                icon: "chart-line".to_string(),
                description: "Time series metrics visualization".to_string(),
            },
            ScreenType::Trace => ScreenTypeInfo {
                name: "trace".to_string(),
                icon: "sitemap".to_string(),
                description: "Performance trace visualization".to_string(),
            },
        }
    }

    /// Returns the default configuration for this screen type.
    pub fn default_config(&self) -> serde_json::Value {
        match self {
            ScreenType::Table => serde_json::json!({
                "sql": "SELECT * FROM processes LIMIT 100",
                "variables": []
            }),
            ScreenType::Metrics => serde_json::json!({
                "sql": "SELECT time, value FROM metrics WHERE $__timeFilter(time) ORDER BY time",
                "variables": []
            }),
            ScreenType::Trace => serde_json::json!({
                "process_id": null,
                "time_range": null,
                "include_async_spans": true,
                "include_thread_spans": true
            }),
        }
    }
}

/// Information about a screen type for display in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenTypeInfo {
    pub name: String,
    pub icon: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_type_serialization() {
        assert_eq!(
            serde_json::to_string(&ScreenType::Table).expect("serialize"),
            "\"table\""
        );
        assert_eq!(
            serde_json::to_string(&ScreenType::Metrics).expect("serialize"),
            "\"metrics\""
        );
        assert_eq!(
            serde_json::to_string(&ScreenType::Trace).expect("serialize"),
            "\"trace\""
        );
    }

    #[test]
    fn test_screen_type_deserialization() {
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"table\"").expect("deserialize"),
            ScreenType::Table
        );
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"metrics\"").expect("deserialize"),
            ScreenType::Metrics
        );
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"trace\"").expect("deserialize"),
            ScreenType::Trace
        );
    }

    #[test]
    fn test_screen_type_from_str() {
        assert_eq!("table".parse::<ScreenType>().unwrap(), ScreenType::Table);
        assert_eq!(
            "metrics".parse::<ScreenType>().unwrap(),
            ScreenType::Metrics
        );
        assert_eq!("trace".parse::<ScreenType>().unwrap(), ScreenType::Trace);
        assert!("invalid".parse::<ScreenType>().is_err());
    }

    #[test]
    fn test_all_screen_types() {
        let all = ScreenType::all();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&ScreenType::Table));
        assert!(all.contains(&ScreenType::Metrics));
        assert!(all.contains(&ScreenType::Trace));
    }
}
