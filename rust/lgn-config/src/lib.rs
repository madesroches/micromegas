//! Configuration.
//!
//! This crate provides a simple configuration system.

mod errors;
mod rich_pathbuf;

use config::{ConfigError, FileFormat};
use once_cell::sync::Lazy;
use std::path::PathBuf;

pub use errors::{Error, Result};
pub use rich_pathbuf::RichPathBuf;

/// The default filename for configuration files.
pub static DEFAULT_FILENAME: &str = "legion.toml";

#[derive(Debug, Clone)]
pub struct Config {
    pub(crate) config: config::Config,
}

pub static CONFIG: Lazy<Config> =
    Lazy::new(|| Config::load().expect("failed to the load the configuration"));

/// Get the value specified by the key.
///
/// If the value does not exist, None is returned.
///
/// # Efficiency
///
/// This method is provided for convenience. If you intend to read several
/// configuration values, it is more efficient to either read a struct or
/// instantiate a `Config` and use the `get` method on the struct.
///
/// # Errors
///
/// If any error occurs, including the specified key not existing in the
/// configuration, it is returned.
pub fn get<'de, T>(key: &str) -> Result<Option<T>>
where
    T: serde::Deserialize<'de>,
{
    (*CONFIG).get(key)
}

/// Get the value specified by the key or a specified default value if it is
/// not found.
///
/// If the value does not exist, the specified default value is returned.
///
/// # Efficiency
///
/// This method is provided for convenience. If you intend to read several
/// configuration values, it is more efficient to either read a struct or
/// instantiate a `Config` and use the `get_or` method on the struct.
///
/// # Errors
///
/// If any other error occurs, it is returned.
pub fn get_or<'de, T>(key: &str, default: T) -> Result<T>
where
    T: serde::Deserialize<'de>,
{
    (*CONFIG).get_or(key, default)
}

/// Get the value specified by the key or builds a default value by calling
/// the specified function if the key is not found.
///
/// # Efficiency
///
/// This method is provided for convenience. If you intend to read several
/// configuration values, it is more efficient to either read a struct or
/// instantiate a `Config` and use the `get_or_else` method on the struct.
///
/// # Errors
///
/// If any other error occurs, it is returned.
pub fn get_or_else<'de, T, F: FnOnce() -> T>(key: &str, f: F) -> Result<T>
where
    T: serde::Deserialize<'de>,
{
    (*CONFIG).get_or_else(key, f)
}

/// Get the value specified by the key or a default value if it is not
/// found.
///
/// If the value does not exist, the default value is returned.
///
/// # Efficiency
///
/// This method is provided for convenience. If you intend to read several
/// configuration values, it is more efficient to either read a struct or
/// instantiate a `Config` and use the `get_or_default` method on the
/// struct.
///
/// # Errors
///
/// If any other error occurs, it is returned.
pub fn get_or_default<'de, T>(key: &str) -> Result<T>
where
    T: serde::Deserialize<'de> + Default,
{
    (*CONFIG).get_or_default(key)
}

impl Config {
    /// Create a configuration from a TOML string.
    ///
    /// Useful for tests mostly.
    pub fn from_toml(toml: &str) -> Self {
        let config = config::Config::builder()
            .add_source(config::File::from_str(toml, config::FileFormat::Toml))
            .build()
            .expect("failed to build the configuration");
        Self { config }
    }

    /// Load the configuration from all its various sources.
    ///
    /// If a configuration value is set in different sources, the value from the
    /// last read source will be used.
    ///
    /// Namely, the configuration will be loaded from the following locations, in
    /// order:
    ///
    /// - `/etc/legion-labs/legion.toml` on UNIX.
    /// - Any `legion.toml` file in the current binary directory, or one of its
    /// parent directories, stopping as soon as a file is found.
    /// - Any `legion.toml` file in the current working directory, or one of its
    /// parent directories, stopping as soon as a file is found. If the first
    /// found file was already read as part of the previous lookup, it is not
    /// read again.
    /// - `$XDG_CONFIG_HOME/legion-labs/legion.toml` on UNIX.
    /// - `$HOME/.config/legion-labs/legion.toml` on UNIX.
    /// - %APPDATA%/legion-labs/legion.toml on Windows.
    /// - Any file specified in the `LGN_CONFIG` environment variable.
    /// - Environment variables, starting with `LGN_`.
    ///
    /// # Errors
    ///
    /// If the configuration cannot be loaded, an error is returned.
    pub fn load() -> Result<Self> {
        let mut config_builder = config::Config::builder();

        // On Unix, always read the system-wide configuration file first if it
        // exists.
        if cfg!(unix) {
            config_builder = config_builder.add_source(
                config::File::with_name(&format!("/etc/legion-labs/{}", DEFAULT_FILENAME))
                    .required(false)
                    .format(FileFormat::Toml),
            );
        }

        // Starting with the current binary directory, walk up to the root,
        // stopping as soon as we find a configuration file.
        let mut known_path = None;
        for dir in std::env::current_exe()?.parent().unwrap().ancestors() {
            let config_file_path = dir.join(DEFAULT_FILENAME);

            if std::fs::metadata(&config_file_path).is_ok() {
                config_builder = config_builder.add_source(
                    config::File::from(config_file_path.clone()).format(FileFormat::Toml),
                );
                known_path = Some(config_file_path);
                break;
            }
        }

        // Then, try to read the closest file we found.
        for dir in std::env::current_dir()?.ancestors() {
            let config_file_path = dir.join(DEFAULT_FILENAME);

            if std::fs::metadata(&config_file_path).is_ok() {
                // If we already loaded that file, do not reload it.
                if let Some(known_path) = known_path {
                    if config_file_path == known_path {
                        break;
                    }
                }
                config_builder = config_builder
                    .add_source(config::File::from(config_file_path).format(FileFormat::Toml));
                break;
            }
        }

        // If we have an user configuration folder, try to read from it.
        if let Some(config_dir) = dirs::config_dir() {
            let config_file_path = config_dir.join("legion-labs").join(DEFAULT_FILENAME);
            config_builder = config_builder.add_source(
                config::File::from(config_file_path)
                    .required(false)
                    .format(FileFormat::Toml),
            );
        }

        // If a specific configuration file was specified, try to read it.
        if let Some(config_file_path) = std::env::var_os("LGN_CONFIG") {
            config_builder = config_builder.add_source(
                config::File::from(PathBuf::from(config_file_path)).format(FileFormat::Toml),
            );
        }

        // Finally, read from environment variables, starting with `LGN`.
        config_builder = config_builder.add_source(config::Environment::with_prefix("LGN"));

        Ok(Self {
            config: config_builder.build()?,
        })
    }

    /// Override this configuration with another one.
    pub fn override_with(&mut self, other: Self) {
        let config = std::mem::take(&mut self.config);
        self.config = config::Config::builder()
            .add_source(config)
            .add_source(other.config)
            .build()
            .expect("failed to build the configuration");
    }

    /// Get the value specified by the key.
    ///
    /// If the value does not exist, None is returned.
    ///
    /// # Errors
    ///
    /// If any error occurs, including the specified key not existing in the
    /// configuration, it is returned.
    pub fn get<'de, T>(&self, key: &str) -> Result<Option<T>>
    where
        T: serde::Deserialize<'de>,
    {
        match self.config.get(key) {
            Ok(value) => Ok(Some(value)),
            Err(err) => match &err {
                ConfigError::NotFound(missing_key) => {
                    if key == missing_key {
                        Ok(None)
                    } else {
                        Err(err.into())
                    }
                }
                _ => Err(err.into()),
            },
        }
    }

    /// Get the value specified by the key or a specified default value if it is
    /// not found.
    ///
    /// If the value does not exist, the specified default value is returned.
    ///
    /// # Errors
    ///
    /// If any other error occurs, it is returned.
    pub fn get_or<'de, T>(&self, key: &str, default: T) -> Result<T>
    where
        T: serde::Deserialize<'de>,
    {
        self.get(key).map(|value| value.unwrap_or(default))
    }

    /// Get the value specified by the key or builds a default value by calling
    /// the specified function if the key is not found.
    ///
    /// # Errors
    ///
    /// If any other error occurs, it is returned.
    pub fn get_or_else<'de, T, F>(&self, key: &str, f: F) -> Result<T>
    where
        T: serde::Deserialize<'de>,
        F: FnOnce() -> T,
    {
        self.get(key).map(|value| value.unwrap_or_else(f))
    }

    /// Get the value specified by the key or a default value if it is not
    /// found.
    ///
    /// If the value does not exist, the default value is returned.
    ///
    /// # Errors
    ///
    /// If any other error occurs, it is returned.
    pub fn get_or_default<'de, T>(&self, key: &str) -> Result<T>
    where
        T: serde::Deserialize<'de> + Default,
    {
        self.get(key).map(Option::unwrap_or_default)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    use lgn_test_utils::jail::Jail;

    #[derive(Serialize, Deserialize, Debug)]
    struct MyConfig {
        my_bool: bool,
        my_int: i64,
        my_float: f64,
        my_list: Vec<String>,
    }

    #[test]
    fn test_load_config_from() {
        Jail::expect_with(|jail| {
            jail.create_file(
                DEFAULT_FILENAME,
                include_str!("test_fixtures/prod_legion.toml"),
            )
            .expect("failed to create jailed file");

            let config = Config::load().unwrap();
            assert_eq!(
                Some("prod"),
                config
                    .get::<String>("lgn-config.tests.environment")
                    .unwrap()
                    .as_deref()
            );
            assert!(config
                .get::<String>("lgn-config.tests.non-existing")
                .unwrap()
                .is_none());
            assert_eq!(
                "",
                config
                    .get_or_default::<String>("lgn-config.tests.non-existing")
                    .unwrap()
            );

            Ok(())
        });
    }

    #[test]
    fn test_load_config_from_with_environment_variable_override() {
        Jail::expect_with(|jail| {
            // Lets set en environment variable, as an override.
            jail.set_env("LGN_LGN-CONFIG.TESTS.ENVIRONMENT", "foo");

            let config = Config::load().unwrap();

            assert_eq!(
                Some("foo"),
                config
                    .get::<String>("lgn-config.tests.environment")
                    .unwrap()
                    .as_deref()
            );

            Ok(())
        });
    }

    #[test]
    fn test_load_config_from_with_struct() {
        Jail::expect_with(|jail| {
            jail.create_file(
                DEFAULT_FILENAME,
                include_str!("test_fixtures/dev_legion.toml"),
            )
            .expect("failed to create jailed file");
            let config = Config::load().unwrap();

            let my_config: MyConfig = config.get("lgn-config.tests.my_config").unwrap().unwrap();

            assert!(my_config.my_bool);
            assert_eq!(42, my_config.my_int);
            assert!(
                (1.23 - my_config.my_float).abs() < std::f64::EPSILON,
                "{} != {}",
                my_config.my_float,
                1.23
            );
            assert_eq!(
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                my_config.my_list
            );
            Ok(())
        });
    }
}
