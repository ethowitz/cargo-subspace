use std::{
    collections::HashMap,
    fmt,
    fs::{self, File},
    io::Read,
};

use anyhow::Result;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use glob::Pattern;
use serde::{Deserialize, Deserializer, de};

use crate::util::Toolchain;

pub const CONFIG_FILE_NAME: &str = "subspace.toml";

#[derive(Debug, Deserialize, Default)]
pub struct SubspaceConfig {
    #[serde(default, deserialize_with = "deserialize_patterns")]
    include: Vec<Pattern>,
    #[serde(default, deserialize_with = "deserialize_patterns")]
    exclude: Vec<Pattern>,
    // // "all", "default", "no-default", or [<LIST OF FEATURES>]
    // features: todo!(),
    #[serde(default)]
    pub workspace: bool,
    #[serde(default)]
    pub crate_config: HashMap<String, CrateConfig>,
    #[serde(default)]
    pub check: CheckConfig,
}

impl SubspaceConfig {
    pub fn find(toolchain: &Toolchain) -> Result<Self> {
        #[derive(Deserialize)]
        struct LocateProjectOutput {
            root: Utf8PathBuf,
        }

        let workspace_path = {
            let output = toolchain
                .cargo()
                .arg("locate-project")
                .arg("--workspace")
                .output()?;

            if output.status.success() {
                let mut root = serde_json::from_slice::<LocateProjectOutput>(&output.stdout)?.root;
                root.pop();
                root
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("Failed to locate workspace root: {stderr}");
            }
        };

        let mut config = if fs::exists(workspace_path.join(CONFIG_FILE_NAME))? {
            let mut toml = String::new();
            let mut file = File::open(workspace_path.join(CONFIG_FILE_NAME))?;
            file.read_to_string(&mut toml)?;

            toml::from_str(&toml)?
        } else {
            Self::default()
        };

        config.include = config
            .include
            .into_iter()
            .map(|pat| Pattern::new(workspace_path.join(pat.as_str()).as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        config.exclude = config
            .exclude
            .into_iter()
            .map(|pat| Pattern::new(workspace_path.join(pat.as_str()).as_str()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(config)
    }

    pub fn uses_explicit_filters(&self) -> bool {
        !self.include.is_empty() || !self.exclude.is_empty()
    }

    pub fn includes_package(&self, manifest_path: &Utf8Path) -> bool {
        let Some(manifest_path) = manifest_path.as_std_path().parent() else {
            return false;
        };

        // If an exclusion pattern matches the given manifest path, the package is not included
        if self
            .exclude
            .iter()
            .any(|pat| pat.matches_path(manifest_path))
        {
            return false;
        }

        // If we're not given any inclusion patterns and the crate does not match any exclusion
        // patterns, the package is included
        if self.include.is_empty() {
            return true;
        }

        // If we're given inclusion patterns, the package is included only if it matches one of the
        // given patterns
        self.include
            .iter()
            .any(|pat| pat.matches_path(manifest_path))
    }
}

fn deserialize_patterns<'de, D>(deserializer: D) -> Result<Vec<Pattern>, D::Error>
where
    D: Deserializer<'de>,
{
    let patterns: Vec<String> = de::Deserialize::deserialize(deserializer)?;

    patterns
        .into_iter()
        .map(|pat| Pattern::new(&pat))
        .collect::<Result<Vec<Pattern>, _>>()
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Default, Deserialize)]
pub struct CrateConfig {
    pub features: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Clone, Copy)]
pub enum CheckCommand {
    #[default]
    Check,
    Clippy,
}

impl fmt::Display for CheckCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Check => write!(f, "check"),
            Self::Clippy => write!(f, "clippy"),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct CheckConfig {
    #[serde(default)]
    pub command: Option<CheckCommand>,
    #[serde(default)]
    pub passthrough_args: Option<Vec<String>>,
    // TODO: add this as CheckArg as well
    pub workspace: Option<bool>,
}
