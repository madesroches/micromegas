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
            "invalid screen type '{}', expected one of: process_list, metrics, log, table, notebook",
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
    Table,
    Notebook,
}

impl FromStr for ScreenType {
    type Err = ParseScreenTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "process_list" => Ok(ScreenType::ProcessList),
            "metrics" => Ok(ScreenType::Metrics),
            "log" => Ok(ScreenType::Log),
            "table" => Ok(ScreenType::Table),
            "notebook" => Ok(ScreenType::Notebook),
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
            ScreenType::Metrics,
            ScreenType::Log,
            ScreenType::Table,
            ScreenType::Notebook,
        ]
    }

    /// Returns the string identifier for this screen type.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScreenType::ProcessList => "process_list",
            ScreenType::Metrics => "metrics",
            ScreenType::Log => "log",
            ScreenType::Table => "table",
            ScreenType::Notebook => "notebook",
        }
    }

    /// Returns information about this screen type.
    pub fn info(&self) -> ScreenTypeInfo {
        match self {
            ScreenType::ProcessList => ScreenTypeInfo {
                name: "process_list".to_string(),
                display_name: "Process List".to_string(),
                icon: "list".to_string(),
                description: "List of processes with filtering".to_string(),
            },
            ScreenType::Metrics => ScreenTypeInfo {
                name: "metrics".to_string(),
                display_name: "Metrics".to_string(),
                icon: "chart-line".to_string(),
                description: "Time series metrics visualization".to_string(),
            },
            ScreenType::Log => ScreenTypeInfo {
                name: "log".to_string(),
                display_name: "Log".to_string(),
                icon: "file-text".to_string(),
                description: "Log entries viewer with filtering".to_string(),
            },
            ScreenType::Table => ScreenTypeInfo {
                name: "table".to_string(),
                display_name: "Table".to_string(),
                icon: "table".to_string(),
                description: "Generic table viewer for any SQL query".to_string(),
            },
            ScreenType::Notebook => ScreenTypeInfo {
                name: "notebook".to_string(),
                display_name: "Notebook".to_string(),
                icon: "book-open".to_string(),
                description: "Multi-cell notebook with tables, charts, logs, and variables"
                    .to_string(),
            },
        }
    }

    /// Returns the default configuration for this screen type.
    pub fn default_config(&self) -> serde_json::Value {
        match self {
            ScreenType::ProcessList => serde_json::json!({
                "timeRangeFrom": "now-5m",
                "timeRangeTo": "now",
                "sql": "SELECT process_id, exe, start_time, last_update_time, username, computer\nFROM processes\n$order_by\nLIMIT 100",
                "variables": []
            }),
            ScreenType::Metrics => serde_json::json!({
                "timeRangeFrom": "now-5m",
                "timeRangeTo": "now",
                "sql": "SELECT time, value\nFROM measures\nWHERE name = 'cpu_usage'\nORDER BY time\nLIMIT 100",
                "variables": []
            }),
            ScreenType::Log => serde_json::json!({
                "timeRangeFrom": "now-5m",
                "timeRangeTo": "now",
                "sql": "SELECT time, level, target, msg\nFROM log_entries\nWHERE level <= $max_level\n  $search_filter\nORDER BY time DESC\nLIMIT $limit",
                "variables": []
            }),
            ScreenType::Table => serde_json::json!({
                "timeRangeFrom": "now-5m",
                "timeRangeTo": "now",
                "sql": "SELECT process_id, exe, start_time, last_update_time, username, computer\nFROM processes\n$order_by\nLIMIT 100",
                "variables": []
            }),
            ScreenType::Notebook => serde_json::json!({
                "timeRangeFrom": "now-5m",
                "timeRangeTo": "now",
                "cells": []
            }),
        }
    }
}

/// Information about a screen type for display in the UI.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenTypeInfo {
    pub name: String,
    pub display_name: String,
    pub icon: String,
    pub description: String,
}
