use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Reserved screen names that cannot be used.
const RESERVED_NAMES: &[&str] = &["new"];

/// A user-defined screen configuration.
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Screen {
    pub name: String,
    pub screen_type: String,
    pub config: serde_json::Value,
    pub created_by: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Request to create a new screen.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateScreenRequest {
    pub name: String,
    pub screen_type: String,
    pub config: serde_json::Value,
}

/// Request to update an existing screen.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateScreenRequest {
    pub config: serde_json::Value,
}

/// Validation error for screen names.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
}

impl ValidationError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.to_string(),
            message: message.to_string(),
        }
    }
}

/// Normalizes a screen name for URL usage.
///
/// - Converts to lowercase
/// - Replaces spaces with hyphens
/// - Removes invalid characters
/// - Collapses consecutive hyphens
pub fn normalize_screen_name(name: &str) -> String {
    let normalized: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c == ' ' { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
        .collect();

    // Collapse consecutive hyphens
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in normalized.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim leading and trailing hyphens
    result.trim_matches('-').to_string()
}

/// Validates a screen name according to the rules:
/// - 3-100 characters
/// - Lowercase letters, numbers, and hyphens only
/// - Must start with a letter
/// - Must end with a letter or number
/// - No consecutive hyphens
/// - Not a reserved name
pub fn validate_screen_name(name: &str) -> Result<(), ValidationError> {
    // Check length
    if name.len() < 3 {
        return Err(ValidationError::new(
            "NAME_TOO_SHORT",
            "Screen name must be at least 3 characters",
        ));
    }
    if name.len() > 100 {
        return Err(ValidationError::new(
            "NAME_TOO_LONG",
            "Screen name must be at most 100 characters",
        ));
    }

    // Check reserved names
    if RESERVED_NAMES.contains(&name) {
        return Err(ValidationError::new(
            "RESERVED_NAME",
            "This screen name is reserved",
        ));
    }

    // Check characters
    let chars: Vec<char> = name.chars().collect();

    // Must start with a letter
    if !chars.first().is_some_and(|c| c.is_ascii_lowercase()) {
        return Err(ValidationError::new(
            "INVALID_START",
            "Screen name must start with a lowercase letter",
        ));
    }

    // Must end with a letter or number
    if !chars
        .last()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err(ValidationError::new(
            "INVALID_END",
            "Screen name must end with a letter or number",
        ));
    }

    // Check all characters are valid and no consecutive hyphens
    let mut prev_hyphen = false;
    for c in &chars {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && *c != '-' {
            return Err(ValidationError::new(
                "INVALID_CHARACTER",
                "Screen name can only contain lowercase letters, numbers, and hyphens",
            ));
        }
        if *c == '-' {
            if prev_hyphen {
                return Err(ValidationError::new(
                    "CONSECUTIVE_HYPHENS",
                    "Screen name cannot contain consecutive hyphens",
                ));
            }
            prev_hyphen = true;
        } else {
            prev_hyphen = false;
        }
    }

    Ok(())
}
