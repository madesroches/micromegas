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
            "invalid screen type '{}', expected one of: process_list, metrics, log",
            self.invalid_value
        )
    }
}

impl std::error::Error for ParseScreenTypeError {}

/// Types of screens that can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenType {
    ProcessList,
    Metrics,
    Log,
}

impl FromStr for ScreenType {
    type Err = ParseScreenTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "process_list" => Ok(ScreenType::ProcessList),
            "metrics" => Ok(ScreenType::Metrics),
            "log" => Ok(ScreenType::Log),
            _ => Err(ParseScreenTypeError {
                invalid_value: s.to_string(),
            }),
        }
    }
}

impl ScreenType {
    /// Returns all available screen types.
    pub fn all() -> Vec<ScreenType> {
        vec![
            ScreenType::ProcessList,
            ScreenType::Metrics,
            ScreenType::Log,
        ]
    }

    /// Returns the string identifier for this screen type.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScreenType::ProcessList => "process_list",
            ScreenType::Metrics => "metrics",
            ScreenType::Log => "log",
        }
    }

    /// Returns information about this screen type.
    pub fn info(&self) -> ScreenTypeInfo {
        match self {
            ScreenType::ProcessList => ScreenTypeInfo {
                name: "process_list".to_string(),
                icon: "list".to_string(),
                description: "List of processes with filtering".to_string(),
            },
            ScreenType::Metrics => ScreenTypeInfo {
                name: "metrics".to_string(),
                icon: "chart-line".to_string(),
                description: "Time series metrics visualization".to_string(),
            },
            ScreenType::Log => ScreenTypeInfo {
                name: "log".to_string(),
                icon: "file-text".to_string(),
                description: "Log entries viewer with filtering".to_string(),
            },
        }
    }

    /// Returns the default configuration for this screen type.
    pub fn default_config(&self) -> serde_json::Value {
        match self {
            ScreenType::ProcessList => serde_json::json!({
                "sql": "SELECT * FROM processes LIMIT 100",
                "variables": []
            }),
            ScreenType::Metrics => serde_json::json!({
                "sql": "SELECT time, value FROM metrics WHERE $__timeFilter(time) ORDER BY time",
                "variables": []
            }),
            ScreenType::Log => serde_json::json!({
                "sql": "SELECT time, level, target, msg FROM log_entries ORDER BY time DESC LIMIT 1000",
                "variables": []
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
            serde_json::to_string(&ScreenType::ProcessList).expect("serialize"),
            "\"process_list\""
        );
        assert_eq!(
            serde_json::to_string(&ScreenType::Metrics).expect("serialize"),
            "\"metrics\""
        );
        assert_eq!(
            serde_json::to_string(&ScreenType::Log).expect("serialize"),
            "\"log\""
        );
    }

    #[test]
    fn test_screen_type_deserialization() {
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"process_list\"").expect("deserialize"),
            ScreenType::ProcessList
        );
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"metrics\"").expect("deserialize"),
            ScreenType::Metrics
        );
        assert_eq!(
            serde_json::from_str::<ScreenType>("\"log\"").expect("deserialize"),
            ScreenType::Log
        );
    }

    #[test]
    fn test_screen_type_from_str() {
        assert_eq!(
            "process_list".parse::<ScreenType>().unwrap(),
            ScreenType::ProcessList
        );
        assert_eq!(
            "metrics".parse::<ScreenType>().unwrap(),
            ScreenType::Metrics
        );
        assert_eq!("log".parse::<ScreenType>().unwrap(), ScreenType::Log);
        assert!("invalid".parse::<ScreenType>().is_err());
    }

    #[test]
    fn test_all_screen_types() {
        let all = ScreenType::all();
        assert_eq!(all.len(), 3);
        assert!(all.contains(&ScreenType::ProcessList));
        assert!(all.contains(&ScreenType::Metrics));
        assert!(all.contains(&ScreenType::Log));
    }
}
