use std::{
    fmt::Display,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};

use regex::{Captures, Regex};
use serde::Deserialize;

use crate::Error;

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RichPathBuf {
    original: String,
    path: PathBuf,
}

impl<'de> Deserialize<'de> for RichPathBuf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(|err| serde::de::Error::custom(format!("invalid path: {}", err)))
    }
}

impl Display for RichPathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.display())
    }
}

impl From<PathBuf> for RichPathBuf {
    fn from(path: PathBuf) -> Self {
        Self {
            original: path.display().to_string(),
            path,
        }
    }
}

impl From<RichPathBuf> for PathBuf {
    fn from(rich_pathbuf: RichPathBuf) -> Self {
        rich_pathbuf.path
    }
}

impl std::ops::Deref for RichPathBuf {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for RichPathBuf {
    fn as_ref(&self) -> &Path {
        self.path.as_ref()
    }
}

impl FromStr for RichPathBuf {
    type Err = Error;

    fn from_str(original: &str) -> Result<Self, Self::Err> {
        // Replace environment variables.
        let re = Regex::new(r"\$\{([a-zA-Z0-9_.-]+)\}").unwrap();
        let s = &re.replace_all(original, |caps: &Captures<'_>| {
            std::env::var(caps.get(1).unwrap().as_str()).unwrap_or_default()
        });

        // Replace commands.
        let re = Regex::new(r"\$\(([a-zA-Z0-9_.-]+)\)").unwrap();
        let s = &re.replace_all(s, |caps: &Captures<'_>| {
            let cmd = caps.get(1).unwrap().as_str();

            match cmd {
                "git-root" => Self::find_git_root(),
                _ => "".to_string(),
            }
        });

        let path = PathBuf::from_str(s).map_err(|e| Error::Other(e.to_string()))?;

        Ok(Self {
            original: original.to_string(),
            path,
        })
    }
}

impl RichPathBuf {
    pub fn original(&self) -> &str {
        &self.original
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    fn find_git_root() -> String {
        match Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    std::str::from_utf8(&output.stdout)
                        .unwrap_or_default()
                        .trim()
                        .to_string()
                } else {
                    "".to_string()
                }
            }
            Err(_) => "".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use lgn_test_utils::jail::Jail;

    use super::*;

    #[test]
    fn test_rich_pathbuf_deserialize() {
        Jail::expect_with(|jail| {
            jail.set_env("MY_SUPER_VARIABLE", "foo");

            let rich_pathbuf: RichPathBuf =
                serde_json::from_value(json!("${MY_SUPER_VARIABLE}/test")).unwrap();

            assert_eq!(rich_pathbuf.original, "${MY_SUPER_VARIABLE}/test");
            assert_eq!(rich_pathbuf.path, PathBuf::from("foo/test"));
            Ok(())
        });
    }
}
