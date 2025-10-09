use std::{
    ffi::OsStr,
    fmt::Display,
    ops::Deref,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::anyhow;
use cargo_metadata::camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Deserializer, Serialize};

/// A wrapper around [`Path`] that can only store a file.
#[derive(PartialEq, Clone, Copy, Debug)]
#[repr(transparent)]
pub(crate) struct FilePath<'a>(&'a Utf8Path);

impl FilePath<'_> {
    pub(crate) fn parent(&self) -> Option<FilePath<'_>> {
        self.0.parent().map(FilePath)
    }
}

impl From<FilePath<'_>> for PathBuf {
    fn from(value: FilePath<'_>) -> Self {
        value.0.into()
    }
}

impl From<FilePath<'_>> for FilePathBuf {
    fn from(value: FilePath<'_>) -> Self {
        FilePathBuf(value.0.into())
    }
}

impl AsRef<OsStr> for FilePath<'_> {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}

impl Deref for FilePath<'_> {
    type Target = Utf8Path;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

/// A wrapper around [`PathBuf`] that can only store a file.
#[derive(PartialEq, Clone, Debug, Serialize)]
#[repr(transparent)]
pub(crate) struct FilePathBuf(Utf8PathBuf);

impl FilePathBuf {
    pub(crate) fn as_file_path(&self) -> FilePath<'_> {
        FilePath(self.0.as_path())
    }
}

impl Display for FilePathBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<Path> for FilePathBuf {
    fn as_ref(&self) -> &Path {
        self.0.as_ref()
    }
}

impl AsRef<OsStr> for FilePathBuf {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}

impl From<FilePathBuf> for Utf8PathBuf {
    fn from(value: FilePathBuf) -> Self {
        value.0
    }
}

impl TryFrom<Utf8PathBuf> for FilePathBuf {
    type Error = anyhow::Error;

    fn try_from(value: Utf8PathBuf) -> Result<Self, Self::Error> {
        if value.is_file() {
            Ok(Self(value))
        } else {
            Err(anyhow!("`{}` is not a file", value))
        }
    }
}

impl TryFrom<PathBuf> for FilePathBuf {
    type Error = anyhow::Error;

    fn try_from(value: PathBuf) -> Result<Self, Self::Error> {
        Utf8PathBuf::from_path_buf(value)
            .map_err(|_| anyhow!("Path contains non-UTF-8 characters"))?
            .try_into()
    }
}

impl FromStr for FilePathBuf {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value: PathBuf = value.into();

        Self::try_from(value)
    }
}

impl Deref for FilePathBuf {
    type Target = Utf8Path;

    fn deref(&self) -> &Self::Target {
        self.0.as_path()
    }
}

impl<'de> Deserialize<'de> for FilePathBuf {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        PathBuf::deserialize(deserializer)?
            .try_into()
            .map_err(|e: anyhow::Error| serde::de::Error::custom(e.to_string()))
    }
}
