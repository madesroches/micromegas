// Credits to figment Jail, https://docs.rs/figment/0.10.5/figment/struct.Jail.html

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fmt::Display,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use tempfile::TempDir;

pub struct Jail {
    _directory: TempDir,
    canonical_dir: PathBuf,
    saved_env_vars: HashMap<OsString, Option<OsString>>,
    saved_cwd: PathBuf,
}

fn as_string<S: Display>(s: S) -> String {
    s.to_string()
}

static JAIL_LOCK: parking_lot::Mutex<()> = parking_lot::const_mutex(());

impl Jail {
    /// Creates a new jail that calls `f`, passing itself to `f`.
    ///
    /// # Panics
    ///
    /// Panics if `f` panics or if [`Jail::try_with(f)`](Jail::try_with) returns
    /// an `Err`; prints the error message.
    ///
    /// # Example
    ///
    /// ```rust
    /// lgn_tests::Jail::expect_with(|jail| {
    ///     /* in the jail */
    ///
    ///     Ok(())
    /// });
    /// ```
    #[track_caller]
    pub fn expect_with<F: FnOnce(&mut Self) -> Result<(), String>>(f: F) {
        if let Err(e) = Self::try_with(f) {
            panic!("jail failed: {}", e)
        }
    }

    /// Creates a new jail that calls `f`, passing itself to `f`. Returns the
    /// result from `f` if `f` does not panic.
    ///
    /// # Panics
    ///
    /// Panics if `f` panics.
    ///
    /// # Errors
    ///
    /// Errors when a temporary file cannot be created.
    ///
    /// # Example
    ///
    /// ```rust
    /// let result = figment::Jail::try_with(|jail| {
    ///     /* in the jail */
    ///
    ///     Ok(())
    /// });
    ///
    /// ```
    #[track_caller]
    pub fn try_with<F: FnOnce(&mut Self) -> Result<(), String>>(f: F) -> Result<(), String> {
        let _lock = JAIL_LOCK.lock();

        let directory = TempDir::new().map_err(as_string)?;
        let mut jail = Self {
            canonical_dir: directory.path().canonicalize().map_err(as_string)?,
            _directory: directory,
            saved_cwd: std::env::current_dir().map_err(as_string)?,
            saved_env_vars: HashMap::new(),
        };

        std::env::set_current_dir(jail.directory()).map_err(as_string)?;
        f(&mut jail)
    }

    /// Returns the directory the jail has switched into. The contents of this
    /// directory will be cleared when `Jail` is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// figment::Jail::expect_with(|jail| {
    ///     let tmp_directory = jail.directory();
    ///
    ///     Ok(())
    /// });
    /// ```
    pub fn directory(&self) -> &Path {
        &self.canonical_dir
    }

    /// Creates a file with contents `contents` in the jail's directory. The
    /// file will be deleted with the jail is dropped.
    ///
    /// # Errors
    ///
    /// Errors when a temporary file cannot be created.
    /// # Example
    ///
    /// ```rust
    /// figment::Jail::expect_with(|jail| {
    ///     jail.create_file("MyConfig.json", "contents...");
    ///     Ok(())
    /// });
    /// ```
    pub fn create_file<P: AsRef<Path>>(&self, path: P, contents: &str) -> Result<File, String> {
        let path = path.as_ref();
        if !path.is_relative() {
            return Err("Jail::create_file(): file path is absolute".to_string());
        }

        let file = File::create(self.directory().join(path)).map_err(as_string)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(contents.as_bytes()).map_err(as_string)?;
        writer.into_inner().map_err(as_string)
    }

    /// Set the environment variable `k` to value `v`. The variable will be
    /// removed when the jail is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// const VAR_NAME: &str = "my-very-special-figment-var";
    ///
    /// assert!(std::env::var(VAR_NAME).is_err());
    ///
    /// figment::Jail::expect_with(|jail| {
    ///     jail.set_env(VAR_NAME, "value");
    ///     assert!(std::env::var(VAR_NAME).is_ok());
    ///     Ok(())
    /// });
    ///
    /// assert!(std::env::var(VAR_NAME).is_err());
    /// ```
    pub fn set_env<K: AsRef<str>, V: Display>(&mut self, k: K, v: V) {
        let key = k.as_ref();
        if !self.saved_env_vars.contains_key(OsStr::new(key)) {
            self.saved_env_vars
                .insert(key.into(), std::env::var_os(key));
        }

        std::env::set_var(key, v.to_string());
    }
}

impl Drop for Jail {
    fn drop(&mut self) {
        for (key, value) in &self.saved_env_vars {
            match value {
                Some(val) => std::env::set_var(key, val),
                None => std::env::remove_var(key),
            }
        }

        let _res = std::env::set_current_dir(&self.saved_cwd);
    }
}
